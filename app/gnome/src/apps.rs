use std::env;
use std::fs;
use std::path::PathBuf;

use gio_unix::DesktopAppInfo;
use gtk::gio::prelude::*;
use lofi_core::Application;

pub fn application_directories() -> Vec<PathBuf> {
    let mut result: Vec<PathBuf> = Vec::new();

    let data_home: Option<PathBuf> = match env::var("XDG_DATA_HOME") {
        Ok(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => match env::var("HOME") {
            Ok(home) if !home.is_empty() => {
                let mut p = PathBuf::from(home);
                p.push(".local");
                p.push("share");
                Some(p)
            }
            _ => None,
        },
    };

    let data_dirs: Vec<PathBuf> = match env::var("XDG_DATA_DIRS") {
        Ok(value) if !value.is_empty() => value
            .split(':')
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect(),
        _ => vec![
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ],
    };

    if let Some(mut p) = data_home {
        p.push("applications");
        result.push(p);
    }

    for mut p in data_dirs {
        p.push("applications");
        result.push(p);
    }

    result
}

pub fn gather_applications(dirs: &[PathBuf]) -> Vec<Application> {
    let mut out: Vec<Application> = Vec::new();

    for dir in dirs {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if !file_type.is_file() {
                continue;
            }

            let path = entry.path();
            let is_desktop = path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.ends_with(".desktop"));
            if !is_desktop {
                continue;
            }

            let info = match DesktopAppInfo::from_filename(&path) {
                Some(info) => info,
                None => continue,
            };

            if !info.should_show() {
                continue;
            }

            let name = info.name().to_string();

            let desktop_id = if let Some(id) = info.id() {
                id.to_string()
            } else if let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
            {
                stem
            } else {
                continue;
            };

            out.push(Application { name, desktop_id });
        }
    }

    out
}
