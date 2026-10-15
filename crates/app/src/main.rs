#![cfg_attr(not(test), windows_subsystem = "windows")]

mod admin;
mod args;
mod batch;
mod demo;
mod diag;
mod dragout;
mod gui;
mod i18n;
mod installer;
mod settings;
mod shell;
mod update;
mod winint;

use i18n::{Key, t, tf};
use installer::Env;
use std::process::ExitCode;

const HELP: &str = "\
Packrat — archive manager

  Packrat.exe [archive ...]        open an archive
  Packrat.exe --demo               made-up data, nothing real is read or written
  Packrat.exe --screenshots DIR    regenerate the README screenshots into DIR
  Packrat.exe --install [--silent] install for the current user (also when named *Setup*.exe)
  Packrat.exe --uninstall [--silent]
  Packrat.exe --update             check GitHub for a newer version and replace this file
  Packrat.exe --extract-here A / --extract-to A / --test A   what the Explorer menu runs
  Packrat.exe --add F / --quick zip|7z|tar.zst F
  Packrat.exe --version
";

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn main() -> ExitCode {
    diag::install_panic_hook();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exe = std::env::current_exe().unwrap_or_default();
    let exe_name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let settings = settings::Settings::load();
    i18n::set_lang(i18n::Lang::from_code(&settings.language));

    if flag(&args, "--version") {
        winint::console_print(&format!("Packrat {}\n", env!("CARGO_PKG_VERSION")));
        return ExitCode::SUCCESS;
    }
    if flag(&args, "--help") || flag(&args, "-h") || flag(&args, "/?") {
        winint::console_print(HELP);
        return ExitCode::SUCCESS;
    }
    if flag(&args, "--update") {
        return ExitCode::from(update::run_headless() as u8);
    }
    if flag(&args, "--uninstall") {
        return run_uninstall(&exe, flag(&args, "--silent"));
    }
    if installer::started_as_installer(&exe_name, &args) {
        return run_install(&exe, flag(&args, "--silent"));
    }

    if installer::is_installed_copy(&Env::real(), &exe) {
        update::cleanup_old(&exe);
    }
    let parsed = args::parse(&args);
    // Explorer starts one process per selected file: the first one collects the others and acts for all
    let action = match parsed.action {
        gui::StartAction::Add(items) => {
            match batch::gather(&dragout::temp_root(), "add", items[0].clone(), batch::SETTLE) {
                Some(all) => gui::StartAction::Add(all),
                None => return ExitCode::SUCCESS,
            }
        }
        gui::StartAction::Quick { format, items } => {
            match batch::gather(
                &dragout::temp_root(),
                &format!("quick-{format}"),
                items[0].clone(),
                batch::SETTLE,
            ) {
                Some(all) => gui::StartAction::Quick { format, items: all },
                None => return ExitCode::SUCCESS,
            }
        }
        other => other,
    };
    let opts = gui::StartOptions {
        open: parsed.open,
        demo: parsed.demo,
        screenshots: parsed.screenshots,
        action,
    };
    dragout::cleanup_stale();
    diag::info(&format!("start {} demo={}", env!("CARGO_PKG_VERSION"), opts.demo));
    match gui::run(opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            diag::error(&format!("window error: {e}"));
            winint::message_box("Packrat", &tf(Key::ErrorGeneric, &[&e.to_string()]), false);
            ExitCode::FAILURE
        }
    }
}

fn run_install(exe: &std::path::Path, silent: bool) -> ExitCode {
    let env = Env::real();
    if !silent
        && !winint::message_box(
            t(Key::AppTitle),
            &format!("{}\n{}", tf(Key::InstallNow, &[]), env.install_dir.display()),
            true,
        )
    {
        return ExitCode::SUCCESS;
    }
    match installer::install(&env, exe) {
        Ok(installed) => {
            diag::info("installed");
            // a silent install (scripts, CI) must not start the program
            if !silent
                && winint::message_box(
                    t(Key::AppTitle),
                    &format!(
                        "{}\n\n{}",
                        tf(Key::Installed, &[&installed.display().to_string()]),
                        t(Key::ContextMenuHint)
                    ),
                    true,
                )
            {
                let _ = std::process::Command::new(&installed).spawn();
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            diag::error(&format!("install failed: {e}"));
            winint::message_box(t(Key::AppTitle), &tf(Key::ErrorGeneric, &[&e]), false);
            ExitCode::FAILURE
        }
    }
}

fn run_uninstall(exe: &std::path::Path, silent: bool) -> ExitCode {
    let env = Env::real();
    if !silent
        && !winint::message_box(
            t(Key::AppTitle),
            &format!("{} {}?", t(Key::UninstallConfirm), env.install_dir.display()),
            true,
        )
    {
        return ExitCode::SUCCESS;
    }
    match installer::uninstall(&env, Some(exe)) {
        Ok(()) => {
            diag::info("uninstalled");
            if !silent {
                winint::message_box(t(Key::AppTitle), t(Key::Uninstalled), false);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            diag::error(&format!("uninstall failed: {e}"));
            winint::message_box(t(Key::AppTitle), &tf(Key::ErrorGeneric, &[&e]), false);
            ExitCode::FAILURE
        }
    }
}
