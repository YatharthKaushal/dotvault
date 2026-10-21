//! share, open and `shares ls|log|revoke`.

use std::process::ExitCode;

use anyhow::{anyhow, bail, Result};
use dotvault_core::api::{
    CreateShareRequest, CreateShareResponse, ListSharesResponse, OpenShareResponse,
    ReportEventRequest, ShareAction, ShareEvent, ShareEventsResponse, MAX_SHARE_PAYLOAD_BYTES,
    SHARE_MAX_TTL_SECS, SHARE_MAX_VIEWS, SHARE_MIN_TTL_SECS,
};
use dotvault_core::crypto::{self, random_key, AAD_SHARE};
use dotvault_core::dotenv::DotEnv;
use dotvault_core::payload::EnvPayload;
use dotvault_core::share_code;
use owo_colors::OwoColorize;
use std::io::Write;

use super::cloud::{check_write_target, find_item, friendly, write_env_file};
use super::vault::default_item_name;
use super::view::print_pairs;
use super::{read_text, resolve_target, Ctx, ResolveOpts};
use crate::cli::{OpenArgs, ShareArgs, SharesCommand};
use crate::client::{normalize_server, ApiError};
use crate::output::{
    age, format_utc, humanize_secs, now_secs, parse_duration, plural, print_json, sanitize, stdout,
    until, Cell, Style, Table,
};
use crate::prompt;

pub fn share(ctx: &Ctx, args: ShareArgs) -> Result<ExitCode> {
    let ttl = parse_duration(&args.expires)?;
    if !(SHARE_MIN_TTL_SECS..=SHARE_MAX_TTL_SECS).contains(&ttl) {
        bail!(
            "--expires must be between {} and {}",
            humanize_secs(SHARE_MIN_TTL_SECS),
            humanize_secs(SHARE_MAX_TTL_SECS)
        );
    }
    if let Some(n) = args.max_views {
        if !(1..=SHARE_MAX_VIEWS).contains(&n) {
            bail!("--max-views must be between 1 and {SHARE_MAX_VIEWS}");
        }
    }

    let cloud = ctx.cloud()?;

    // Where the variables come from: a vault item or a local file.
    let (source, full) = match &args.vault {
        Some(name) => {
            let vault_key = cloud.vault_key()?;
            let (items, _) = cloud.items(&vault_key)?;
            let item = find_item(&items, name)?;
            (item.name.clone(), cloud.read_item(&vault_key, item)?)
        }
        None => {
            let path = resolve_target(
                ctx,
                args.target.as_deref(),
                &args.flags,
                ResolveOpts {
                    include_examples: false,
                    may_create: false,
                },
            )?;
            let env = DotEnv::parse(&read_text(&path, false)?);
            let source = default_item_name(ctx, &path);
            (
                source.clone(),
                EnvPayload::from_dotenv(&env, source, None).0,
            )
        }
    };

    // Which variables to share.
    let mut payload = if args.interactive {
        let names: Vec<String> = full.entries.iter().map(|e| e.key.clone()).collect();
        let picked = prompt::select("Variables to share:", &names)?;
        let keys: Vec<String> = picked.into_iter().map(|i| names[i].clone()).collect();
        full.select(&keys).0
    } else if args.keys.is_empty() {
        if full.entries.len() > 1 {
            anstream::eprintln!(
                "{}",
                format!(
                    "sharing all {} variables; use --keys A,B or -i to share only some",
                    full.entries.len()
                )
                .dimmed()
            );
        }
        full
    } else {
        let (picked, missing) = full.select(&args.keys);
        if !missing.is_empty() {
            bail!("not set in {source}: {}", missing.join(", "));
        }
        picked
    };
    if payload.entries.is_empty() {
        bail!("there is nothing to share");
    }
    payload.source = source;
    payload.note = args.note.filter(|n| !n.trim().is_empty());

    // The key never reaches the server; it lives only in the link.
    let share_key = random_key()?;
    let sealed = crypto::seal_json(&share_key, &payload, AAD_SHARE)?;
    if sealed.len() > MAX_SHARE_PAYLOAD_BYTES {
        bail!(
            "that is too large to share ({} KiB, the limit is {} KiB); pick fewer variables",
            sealed.len() / 1024,
            MAX_SHARE_PAYLOAD_BYTES / 1024
        );
    }
    let created: CreateShareResponse = cloud
        .api
        .post(
            "/shares",
            &CreateShareRequest {
                payload_ct: sealed,
                ttl_secs: Some(ttl),
                max_views: args.max_views,
            },
        )
        .map_err(friendly)?;

    let link = share_code::encode_link(cloud.api.base(), &created.id, &share_key);
    let code = share_code::encode_code(&created.id, &share_key);
    let names: Vec<&str> = payload.entries.iter().map(|e| e.key.as_str()).collect();

    // Summary on stderr, the link alone on stdout so it can be piped or copied.
    anstream::eprintln!(
        "Shared {}: {}",
        plural(names.len(), "variable"),
        names.join(", ")
    );
    anstream::eprintln!(
        "  expires  {} ({}){}",
        format_utc(created.expires_at),
        until(now_secs(), created.expires_at),
        args.max_views
            .map(|n| format!(" · max {n} {}", if n == 1 { "open" } else { "opens" }))
            .unwrap_or_default()
    );
    anstream::eprintln!("  code     {code}");
    anstream::eprintln!(
        "{}",
        "The recipient needs a dotvault account: `dotvault open <link or code>`".dimmed()
    );
    anstream::println!("{link}");
    Ok(ExitCode::SUCCESS)
}

pub fn open(ctx: &Ctx, args: OpenArgs) -> Result<ExitCode> {
    // Opening uses up a view, so refuse the useless invocation up front.
    if !args.reveal && args.write.is_none() && args.save.is_none() {
        bail!("say what to do with it: --reveal to print, --write PATH to write a file, or --save to keep it in your vault (nothing was opened)");
    }
    let target = share_code::parse(&args.share)?;
    let cloud = ctx.cloud()?;

    // Never send our token to a server other than the one we logged in to.
    if let Some(origin) = &target.origin {
        let origin = normalize_server(origin)?;
        if origin != cloud.api.base() {
            bail!(
                "this link is for {origin}, but you are logged in to {}; log in there first: dotvault login --server {origin}",
                cloud.api.base()
            );
        }
    }

    // Anything that can fail locally is checked first so a view is not wasted.
    if let Some(path) = &args.write {
        check_write_target(path, args.force, args.merge)?;
    }
    let vault_key = match args.save {
        Some(_) => Some(cloud.vault_key()?),
        None => None,
    };

    let opened: OpenShareResponse = cloud
        .api
        .post_empty(&format!("/shares/{}/open", target.id))
        .map_err(|e| match e {
            ApiError::Http { status: 404, .. } => {
                anyhow!("there is no such share; check that the whole link or code was copied")
            }
            ApiError::Http { ref code, .. } if code == "share_gone" => {
                anyhow!("this share has expired, was revoked, or reached its open limit")
            }
            e => friendly(e),
        })?;
    let payload: EnvPayload = crypto::open_json(&target.key, &opened.payload_ct, AAD_SHARE)
        .map_err(|_| {
            anyhow!("could not decrypt the share; the key part of the link or code is wrong")
        })?;

    // The sender controls the source label and the note: both are sanitized before display.
    let mut header = format!("From {}", sanitize(&opened.owner).bold());
    if !payload.source.is_empty() {
        header.push_str(&format!(" · {}", sanitize(&payload.source)));
    }
    header.push_str(&format!(" · {}", until(now_secs(), opened.expires_at)));
    if let Some(left) = opened.views_left {
        header.push_str(&format!(" · {} left", plural(left as usize, "open")));
    }
    anstream::eprintln!("{header}");
    if let Some(note) = &payload.note {
        anstream::eprintln!("Note: {}", sanitize(note));
    }

    let pairs: Vec<(String, String)> = payload
        .entries
        .iter()
        .map(|e| (e.key.clone(), e.value.clone()))
        .collect();
    if args.reveal {
        print_pairs(&plural(pairs.len(), "variable"), &pairs, args.format, true)?;
    }
    if let Some(path) = &args.write {
        anstream::println!(
            "{}",
            write_env_file(path, &payload, args.force, args.merge)?
        );
    }
    if let (Some(name), Some(vault_key)) = (&args.save, &vault_key) {
        let name = name.clone().unwrap_or_else(|| {
            let what = if payload.source.is_empty() {
                target.id[..8].to_string()
            } else {
                sanitize(&payload.source).into_owned()
            };
            format!("shared/{}/{what}", sanitize(&opened.owner))
        });
        let (items, _) = cloud.items(vault_key)?;
        let existing = items.iter().find(|i| i.name == name);
        let (_, version) = cloud.write_item(vault_key, existing, &name, &payload)?;
        anstream::println!(
            "Saved to your vault as {} (v{version})",
            sanitize(&name).bold()
        );
        // Lets the owner see the save. The server cannot check it; it is a courtesy report.
        if let Err(e) = cloud.api.post_unit(
            &format!("/shares/{}/events", target.id),
            &ReportEventRequest {
                action: ShareAction::Save,
            },
        ) {
            anstream::eprintln!(
                "{} could not report the save to the owner: {}",
                "warning:".yellow().bold(),
                sanitize(&e.to_string())
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn shares(ctx: &Ctx, command: SharesCommand) -> Result<ExitCode> {
    let cloud = ctx.cloud()?;
    match command {
        SharesCommand::Ls { json } => {
            let list: ListSharesResponse = cloud.api.get("/shares").map_err(friendly)?;
            if json {
                print_json(&list.shares)?;
            } else if list.shares.is_empty() {
                anstream::eprintln!("You have not created any shares. Try `dotvault share`.");
            } else {
                let now = now_secs();
                let mut table = Table::new(&["ID", "CREATED", "EXPIRES", "OPENS", "STATUS"]);
                for s in &list.shares {
                    let (status, style) = if s.revoked {
                        ("revoked", Style::Yellow)
                    } else if s.expired {
                        ("expired", Style::Dim)
                    } else {
                        ("active", Style::Cyan)
                    };
                    table.row(vec![
                        s.id.clone().into(),
                        Cell::new(age(now, s.created_at), Style::Dim),
                        Cell::new(until(now, s.expires_at), Style::Dim),
                        match s.max_views {
                            Some(max) => format!("{}/{max}", s.view_count),
                            None => s.view_count.to_string(),
                        }
                        .into(),
                        Cell::new(status, style),
                    ]);
                }
                table.render(&mut stdout().lock())?;
            }
        }
        SharesCommand::Log { id, json } => {
            let id = resolve_share_id(&cloud.api, &id)?;
            let log: ShareEventsResponse = cloud
                .api
                .get(&format!("/shares/{id}/events"))
                .map_err(friendly)?;
            if json {
                print_json(&log.events)?;
            } else if log.events.is_empty() {
                anstream::eprintln!("Nobody has opened this share yet.");
            } else {
                let mut table = Table::new(&["WHEN", "USER", "ACTION", "CLIENT"]);
                for e in &log.events {
                    table.row(vec![
                        format_utc(e.created_at).into(),
                        Cell::new(&e.user, Style::Bold),
                        Cell::new(
                            action_name(e),
                            if e.action == ShareAction::Save {
                                Style::Cyan
                            } else {
                                Style::Plain
                            },
                        ),
                        Cell::new(e.client.clone().unwrap_or_default(), Style::Dim),
                    ]);
                }
                table.render(&mut stdout().lock())?;
            }
        }
        SharesCommand::Revoke { id } => {
            let id = resolve_share_id(&cloud.api, &id)?;
            cloud
                .api
                .delete(&format!("/shares/{id}"))
                .map_err(friendly)?;
            writeln!(stdout().lock(), "Revoked {id}. The link no longer works.")?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn action_name(e: &ShareEvent) -> &'static str {
    match e.action {
        ShareAction::View => "opened",
        ShareAction::Save => "saved",
    }
}

/// Accepts a full id or a unique prefix of one of your shares.
fn resolve_share_id(api: &crate::client::Api, input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        bail!("give a share id (see `dotvault shares ls`)");
    }
    if input.len() >= 22 {
        return Ok(input.to_string());
    }
    let list: ListSharesResponse = api.get("/shares").map_err(friendly)?;
    let hits: Vec<&str> = list
        .shares
        .iter()
        .filter(|s| s.id.starts_with(input))
        .map(|s| s.id.as_str())
        .collect();
    match hits.as_slice() {
        [one] => Ok((*one).to_string()),
        [] => bail!("none of your shares starts with {input:?}; see `dotvault shares ls`"),
        _ => bail!("{input:?} matches several shares; type more of the id"),
    }
}
