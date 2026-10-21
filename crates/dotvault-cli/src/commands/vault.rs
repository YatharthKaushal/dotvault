//! push, pull and `vault ls|show|rm`.

use std::io::Write;
use std::path::{self, Path};
use std::process::ExitCode;

use anyhow::{bail, Result};
use dotvault_core::classify::{
    classify, find_project_root, project_name, ClassifyOptions, PROJECT_MARKERS,
};
use dotvault_core::dotenv::DotEnv;
use dotvault_core::payload::EnvPayload;
use owo_colors::OwoColorize;
use serde::Serialize;

use super::cloud::{check_write_target, find_item, friendly, write_env_file};
use super::view::print_pairs;
use super::{read_text, resolve_target, Ctx, ResolveOpts};
use crate::cli::{PullArgs, PushArgs, VaultCommand};
use crate::output::{
    age, now_secs, plural, print_json, render_pairs_warn, sanitize, stdout, Cell, Format, Style,
    Table,
};
use crate::prompt;
use crate::scan::classify_options;

/// `project/env`, e.g. `acme-api/production`. The project is the nearest
/// folder with a project marker, but never the home directory or above.
pub(super) fn default_item_name(ctx: &Ctx, path: &Path) -> String {
    let abs = path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let file_name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let opts: ClassifyOptions = classify_options(&ctx.config);
    let label = classify(&file_name, &opts).map_or(file_name.clone(), |i| i.label);

    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    let parent = abs.parent().unwrap_or(&abs);
    let root = find_project_root(parent, |dir| {
        if home.as_deref().is_some_and(|h| h.starts_with(dir)) {
            return false;
        }
        PROJECT_MARKERS.iter().any(|m| dir.join(m).exists())
    })
    .unwrap_or_else(|| parent.to_path_buf());
    // Names are shown in many places; a hostile file name must not smuggle in control characters.
    sanitize(&format!("{}/{label}", project_name(&root))).into_owned()
}

fn filter_keys(payload: EnvPayload, keys: &[String], what: &str) -> Result<EnvPayload> {
    if keys.is_empty() {
        return Ok(payload);
    }
    let (picked, missing) = payload.select(keys);
    if !missing.is_empty() {
        bail!("not set in {what}: {}", missing.join(", "));
    }
    Ok(picked)
}

pub fn push(ctx: &Ctx, args: PushArgs) -> Result<ExitCode> {
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
    let name = args.name.unwrap_or_else(|| default_item_name(ctx, &path));
    if name.trim().is_empty() || name.chars().count() > 200 {
        bail!("the name must be 1-200 characters");
    }

    let keys: Option<&[String]> = (!args.keys.is_empty()).then_some(&args.keys[..]);
    let (payload, missing) = EnvPayload::from_dotenv(&env, name.clone(), keys);
    if !missing.is_empty() {
        bail!("not set in {}: {}", path.display(), missing.join(", "));
    }
    if payload.entries.is_empty() {
        bail!("{} defines no variables; nothing to push", path.display());
    }

    let cloud = ctx.cloud()?;
    let vault_key = cloud.vault_key()?;
    let (items, _) = cloud.items(&vault_key)?;
    let existing = items.iter().find(|i| i.name == name);
    let (_, version) = cloud.write_item(&vault_key, existing, &name, &payload)?;
    anstream::println!(
        "Pushed {} (v{version}, {})",
        name.bold(),
        plural(payload.entries.len(), "variable")
    );
    Ok(ExitCode::SUCCESS)
}

pub fn pull(ctx: &Ctx, args: PullArgs) -> Result<ExitCode> {
    if args.out.is_none() && !args.stdout {
        bail!(
            "say where to put it: --out PATH to write a file, or --stdout to print the variables"
        );
    }
    if let Some(out) = &args.out {
        // Fail before any network or password prompt.
        check_write_target(out, args.force, args.merge)?;
    }

    let cloud = ctx.cloud()?;
    let vault_key = cloud.vault_key()?;
    let (items, _) = cloud.items(&vault_key)?;
    let item = find_item(&items, &args.name)?;
    let payload = filter_keys(cloud.read_item(&vault_key, item)?, &args.keys, &item.name)?;

    match &args.out {
        Some(out) => {
            anstream::println!("{}", write_env_file(out, &payload, args.force, args.merge)?);
        }
        None => {
            let pairs: Vec<(String, String)> = payload
                .entries
                .iter()
                .map(|e| (e.key.clone(), e.value.clone()))
                .collect();
            write!(
                stdout().lock(),
                "{}",
                render_pairs_warn(Format::Dotenv, &pairs, true)
            )?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn vault(ctx: &Ctx, command: VaultCommand) -> Result<ExitCode> {
    match command {
        VaultCommand::Ls { json } => {
            let cloud = ctx.cloud()?;
            let vault_key = cloud.vault_key()?;
            let (items, undecryptable) = cloud.items(&vault_key)?;

            if json {
                #[derive(Serialize)]
                struct Row<'a> {
                    id: &'a str,
                    name: &'a str,
                    version: u32,
                    updated_at: i64,
                }
                let rows: Vec<Row> = items
                    .iter()
                    .map(|i| Row {
                        id: &i.id,
                        name: &i.name,
                        version: i.version,
                        updated_at: i.updated_at,
                    })
                    .collect();
                print_json(&rows)?;
            } else if items.is_empty() {
                anstream::eprintln!("Your vault is empty. Store a file with `dotvault push`.");
            } else {
                let now = now_secs();
                let mut table = Table::new(&["NAME", "VERSION", "UPDATED"]);
                for i in &items {
                    table.row(vec![
                        Cell::new(&i.name, Style::Bold),
                        format!("v{}", i.version).into(),
                        Cell::new(age(now, i.updated_at), Style::Dim),
                    ]);
                }
                table.render(&mut stdout().lock())?;
            }
            if undecryptable > 0 {
                anstream::eprintln!(
                    "{} {undecryptable} items could not be decrypted with your key and are hidden",
                    "warning:".yellow().bold()
                );
            }
        }
        VaultCommand::Show {
            name,
            reveal,
            format,
            keys,
        } => {
            let cloud = ctx.cloud()?;
            let vault_key = cloud.vault_key()?;
            let (items, _) = cloud.items(&vault_key)?;
            let item = find_item(&items, &name)?;
            let payload = filter_keys(cloud.read_item(&vault_key, item)?, &keys, &item.name)?;
            let pairs: Vec<(String, String)> = payload
                .entries
                .iter()
                .map(|e| (e.key.clone(), e.value.clone()))
                .collect();
            print_pairs(
                &format!("vault: {} (v{})", item.name, item.version),
                &pairs,
                format,
                reveal,
            )?;
        }
        VaultCommand::Rm { name, yes } => {
            let cloud = ctx.cloud()?;
            let vault_key = cloud.vault_key()?;
            let (items, _) = cloud.items(&vault_key)?;
            let item = find_item(&items, &name)?;
            if !yes && !prompt::confirm(&format!("Delete {:?} from your vault?", item.name))? {
                bail!("not deleted (pass --yes to confirm without a prompt)");
            }
            cloud
                .api
                .delete(&format!("/vault/items/{}", item.id))
                .map_err(friendly)?;
            anstream::println!("Deleted {}", sanitize(&item.name));
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Dirs};
    use std::fs;

    fn ctx() -> Ctx {
        let dir = std::path::PathBuf::from("/nonexistent-dotvault-test");
        Ctx {
            dirs: Dirs {
                config: dir.clone(),
                data: dir,
            },
            config: Config::default(),
        }
    }

    #[test]
    fn names_items_after_project_and_env() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("acme-api");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(project.join("package.json"), "{}").unwrap();
        fs::write(project.join(".env.production"), "A=1\n").unwrap();
        fs::write(project.join(".env"), "A=1\n").unwrap();
        fs::write(project.join(".env.local.example"), "").unwrap();

        let ctx = ctx();
        assert_eq!(
            default_item_name(&ctx, &project.join(".env.production")),
            "acme-api/production"
        );
        assert_eq!(
            default_item_name(&ctx, &project.join(".env")),
            "acme-api/default"
        );
        assert_eq!(
            default_item_name(&ctx, &project.join(".env.local.example")),
            "acme-api/local.example"
        );
    }

    #[test]
    fn names_unmarked_folders_after_the_directory_and_custom_files_after_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let loose = tmp.path().join("loose");
        fs::create_dir_all(&loose).unwrap();
        let ctx = ctx();
        assert_eq!(
            default_item_name(&ctx, &loose.join(".env")),
            "loose/default"
        );
        assert_eq!(
            default_item_name(&ctx, &loose.join("secrets.env")),
            "loose/secrets.env"
        );
    }

    #[test]
    fn filtering_keys_reports_missing_ones() {
        let (p, _) = EnvPayload::from_dotenv(&DotEnv::parse("A=1\nB=2\n"), "s", None);
        let ok = filter_keys(p.clone(), &["B".to_string()], "x").unwrap();
        assert_eq!(ok.entries.len(), 1);
        let err = filter_keys(p.clone(), &["B".to_string(), "Q".to_string()], "thing")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not set in thing: Q"), "{err}");
        assert_eq!(filter_keys(p, &[], "x").unwrap().entries.len(), 2);
    }
}
