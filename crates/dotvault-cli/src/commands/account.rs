//! register, login, logout, whoami, passwd, recover.

use std::io::Write;
use std::process::ExitCode;

use anyhow::{anyhow, bail, Result};
use dotvault_core::api::{
    ChangePasswordRequest, LoginRequest, MeResponse, PreloginResponse, RecoverRequest,
    RecoverUnlockRequest, RecoverUnlockResponse, RegisterRequest, SessionResponse,
};
use dotvault_core::crypto::{
    create_account, derive_account_keys, derive_recovery_keys, format_recovery_key,
    parse_recovery_key, rewrap_for_password, unlock_with_password, unlock_with_recovery, KdfParams,
    Key,
};
use owo_colors::OwoColorize;

use super::cloud::friendly;
use super::Ctx;
use crate::cli::{LoginArgs, RecoverArgs, RegisterArgs};
use crate::client::{encode_query, Api, ApiError};
use crate::output::{format_utc, stdout};
use crate::prompt;
use crate::session::Session;

fn device_name() -> Option<String> {
    ["HOSTNAME", "COMPUTERNAME"]
        .iter()
        .find_map(|v| std::env::var(v).ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
}

fn username_or_prompt(given: Option<String>, label: &str) -> Result<String> {
    let name = match given {
        Some(n) => n,
        None => prompt::line(label, Some("DOTVAULT_USERNAME"))?,
    };
    let name = name.trim().to_string();
    if name.is_empty() {
        bail!("a username is required");
    }
    Ok(name)
}

fn working(message: &str) {
    anstream::eprintln!("{}", message.dimmed());
}

/// Argon2 on the server's parameters. A hostile server must not be able to
/// make us run a trivially weak or enormous derivation, hence the validation.
fn prelogin(api: &Api, username: &str) -> Result<PreloginResponse> {
    let pre: PreloginResponse = api
        .get(&format!(
            "/auth/prelogin?username={}",
            encode_query(username)
        ))
        .map_err(friendly)?;
    pre.kdf_params.validate().map_err(|_| {
        anyhow!("the server asked for unreasonable key-derivation settings; refusing to continue")
    })?;
    Ok(pre)
}

fn save_session(
    ctx: &mut Ctx,
    server: &str,
    username: &str,
    resp: SessionResponse,
    salt: Vec<u8>,
    params: KdfParams,
    cached_key: Option<&Key>,
) -> Result<()> {
    let mut session = Session::new(
        server.to_string(),
        username.to_ascii_lowercase(),
        resp.token,
        resp.expires_at,
        resp.wrapped_vault_key,
        salt,
        params,
    );
    session.cache_vault_key(cached_key);
    ctx.session_store().save(&session)?;
    ctx.remember_server(server);
    Ok(())
}

pub fn register(ctx: &mut Ctx, args: RegisterArgs) -> Result<ExitCode> {
    let server = ctx.pick_server(args.server.as_deref())?;
    let username = username_or_prompt(args.username, "Choose a username: ")?;
    prompt::check_username(&username)?;
    let password = prompt::new_password(
        "Choose a password: ",
        "Repeat the password: ",
        "DOTVAULT_PASSWORD",
    )?;
    prompt::check_new_password(&password, &username)?;

    working("Deriving your keys...");
    let account = create_account(&password, KdfParams::default())?;
    let api = Api::new(&server, None)?;
    let resp: SessionResponse = api
        .post(
            "/auth/register",
            &RegisterRequest {
                username: username.clone(),
                kdf_salt: account.password.kdf_salt.to_vec(),
                kdf_params: account.password.kdf_params,
                auth_key: account.password.auth_key.to_vec(),
                wrapped_vault_key: account.password.wrapped_vault_key.clone(),
                recovery_auth_key: account.recovery_auth_key.to_vec(),
                wrapped_vault_key_recovery: account.wrapped_vault_key_recovery.clone(),
            },
        )
        .map_err(|e| match e.code() {
            Some("username_taken") => anyhow!("the username {username:?} is already taken"),
            _ => friendly(e),
        })?;

    save_session(
        ctx,
        &server,
        &username,
        resp,
        account.password.kdf_salt.to_vec(),
        account.password.kdf_params,
        Some(&account.vault_key),
    )?;

    let mut out = stdout().lock();
    writeln!(
        out,
        "Account created. You are logged in as {} on {server}.",
        username.to_ascii_lowercase().bold()
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "{}",
        "YOUR RECOVERY KEY - write it down and keep it somewhere safe:".bold()
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "    {}",
        format_recovery_key(&account.recovery_key).yellow().bold()
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "It is shown only once. If you forget your password, this key is the ONLY"
    )?;
    writeln!(
        out,
        "way back into your cloud vault: nobody, including the server, can reset it."
    )?;
    Ok(ExitCode::SUCCESS)
}

pub fn login(ctx: &mut Ctx, args: LoginArgs) -> Result<ExitCode> {
    let server = ctx.pick_server(args.server.as_deref())?;
    let username = username_or_prompt(args.username, "Username: ")?;
    let password = prompt::password("Password: ", "DOTVAULT_PASSWORD")?;

    let api = Api::new(&server, None)?;
    let pre = prelogin(&api, &username)?;
    working("Unlocking...");
    let keys = derive_account_keys(&password, &pre.kdf_salt, &pre.kdf_params)?;
    let resp: SessionResponse = api
        .post(
            "/auth/login",
            &LoginRequest {
                username: username.clone(),
                auth_key: keys.auth_key.to_vec(),
                device_name: device_name(),
            },
        )
        .map_err(friendly)?;
    let vault_key = unlock_with_password(&keys, &resp.wrapped_vault_key).map_err(|_| {
        anyhow!("the server accepted the login but sent a vault key that does not unlock; the server may be misbehaving")
    })?;

    save_session(
        ctx,
        &server,
        &username,
        resp,
        pre.kdf_salt,
        pre.kdf_params,
        (!args.no_cache_key).then_some(&vault_key),
    )?;
    anstream::println!(
        "Logged in as {} on {server}.",
        username.to_ascii_lowercase()
    );
    if args.no_cache_key {
        anstream::println!(
            "The vault key is not stored; vault commands will ask for your password."
        );
    }
    Ok(ExitCode::SUCCESS)
}

pub fn logout(ctx: &Ctx) -> Result<ExitCode> {
    let store = ctx.session_store();
    let Some(session) = store.load()? else {
        anstream::println!("You are not logged in.");
        return Ok(ExitCode::SUCCESS);
    };
    // Revoke server-side so a copied token is useless. Best effort.
    let revoked = Api::new(&session.server, Some(session.token.clone()))
        .and_then(|api| {
            api.post_empty_unit("/auth/logout")
                .map_err(anyhow::Error::new)
        })
        .is_ok();
    store.clear()?;
    if revoked {
        anstream::println!("Logged out.");
    } else {
        anstream::println!("Logged out locally. The server could not be reached, so the session stays valid there until it expires.");
    }
    Ok(ExitCode::SUCCESS)
}

pub fn whoami(ctx: &Ctx) -> Result<ExitCode> {
    let cloud = ctx.cloud()?;
    let me: MeResponse = cloud.api.get("/me").map_err(friendly)?;
    let mut out = stdout().lock();
    writeln!(out, "{} on {}", me.username.bold(), cloud.session.server)?;
    writeln!(out, "member since {}", format_utc(me.created_at))?;
    writeln!(
        out,
        "vault key: {}",
        if cloud.session.cached_vault_key().is_some() {
            "stored on this device"
        } else if cloud.session.has_vault_material() {
            "not stored; commands ask for your password"
        } else {
            "unavailable (session from DOTVAULT_TOKEN)"
        }
    )?;
    Ok(ExitCode::SUCCESS)
}

pub fn passwd(ctx: &Ctx) -> Result<ExitCode> {
    let cloud = ctx.cloud()?;
    if !cloud.session.has_vault_material() {
        bail!("changing the password needs a full login; run `dotvault login`");
    }
    let username = cloud.session.username.clone();
    let current = prompt::password("Current password: ", "DOTVAULT_PASSWORD")?;
    let new = prompt::new_password(
        "New password: ",
        "Repeat the new password: ",
        "DOTVAULT_NEW_PASSWORD",
    )?;
    prompt::check_new_password(&new, &username)?;

    // Salt and parameters as the server has them now.
    let pre = prelogin(&cloud.api, &username)?;
    working("Checking your current password...");
    let keys = derive_account_keys(&current, &pre.kdf_salt, &pre.kdf_params)?;
    let vault_key = unlock_with_password(&keys, &cloud.session.wrapped_vault_key).map_err(|_| {
        anyhow!("wrong current password, or the password was changed on another device (run `dotvault login`)")
    })?;

    working("Deriving the new keys...");
    let fresh = rewrap_for_password(&new, KdfParams::default(), &vault_key)?;
    cloud
        .api
        .post_unit(
            "/auth/password",
            &ChangePasswordRequest {
                current_auth_key: keys.auth_key.to_vec(),
                new_kdf_salt: fresh.kdf_salt.to_vec(),
                new_kdf_params: fresh.kdf_params,
                new_auth_key: fresh.auth_key.to_vec(),
                new_wrapped_vault_key: fresh.wrapped_vault_key.clone(),
            },
        )
        .map_err(friendly)?;

    let mut session = cloud.session;
    session.wrapped_vault_key = fresh.wrapped_vault_key;
    session.kdf_salt = fresh.kdf_salt.to_vec();
    session.kdf_params = fresh.kdf_params;
    ctx.session_store().save(&session)?;
    anstream::println!("Password changed. Your other devices were logged out.");
    Ok(ExitCode::SUCCESS)
}

pub fn recover(ctx: &mut Ctx, args: RecoverArgs) -> Result<ExitCode> {
    let server = ctx.pick_server(args.server.as_deref())?;
    let username = username_or_prompt(args.username, "Username: ")?;
    let recovery_text = prompt::line("Recovery key: ", Some("DOTVAULT_RECOVERY_KEY"))?;
    let recovery_key = parse_recovery_key(&recovery_text)
        .map_err(|_| anyhow!("that does not look like a recovery key (it starts with DVR1-)"))?;
    let recovery = derive_recovery_keys(&recovery_key)?;

    let api = Api::new(&server, None)?;
    let unlocked: RecoverUnlockResponse = api
        .post(
            "/auth/recover/unlock",
            &RecoverUnlockRequest {
                username: username.clone(),
                recovery_auth_key: recovery.auth_key.to_vec(),
            },
        )
        .map_err(|e| match e {
            ApiError::Http { ref code, .. } if code == "invalid_credentials" => {
                anyhow!("wrong username or recovery key")
            }
            e => friendly(e),
        })?;
    let vault_key = unlock_with_recovery(&recovery, &unlocked.wrapped_vault_key_recovery)
        .map_err(|_| anyhow!("the recovery key matched but could not unlock the vault; the server may be misbehaving"))?;

    let new = prompt::new_password(
        "New password: ",
        "Repeat the new password: ",
        "DOTVAULT_NEW_PASSWORD",
    )?;
    prompt::check_new_password(&new, &username)?;
    working("Deriving the new keys...");
    let fresh = rewrap_for_password(&new, KdfParams::default(), &vault_key)?;

    api.post_unit(
        "/auth/recover",
        &RecoverRequest {
            username: username.clone(),
            recovery_auth_key: recovery.auth_key.to_vec(),
            new_kdf_salt: fresh.kdf_salt.to_vec(),
            new_kdf_params: fresh.kdf_params,
            new_auth_key: fresh.auth_key.to_vec(),
            new_wrapped_vault_key: fresh.wrapped_vault_key.clone(),
        },
    )
    .map_err(friendly)?;

    // We already hold the new auth key, so logging in needs no second derivation.
    let resp: SessionResponse = api
        .post(
            "/auth/login",
            &LoginRequest {
                username: username.clone(),
                auth_key: fresh.auth_key.to_vec(),
                device_name: device_name(),
            },
        )
        .map_err(friendly)?;
    save_session(
        ctx,
        &server,
        &username,
        resp,
        fresh.kdf_salt.to_vec(),
        fresh.kdf_params,
        (!args.no_cache_key).then_some(&vault_key),
    )?;
    anstream::println!(
        "Password reset. You are logged in as {}. Every other session was ended.",
        username.to_ascii_lowercase()
    );
    Ok(ExitCode::SUCCESS)
}
