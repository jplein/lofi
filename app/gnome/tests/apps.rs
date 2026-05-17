use lofi_gnome::{Application, gather_applications};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn write_desktop(dir: &Path, filename: &str, name: &str, exec: &str, icon: &str) {
    fs::create_dir_all(dir).expect("create_dir_all should succeed for test fixture dir");
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\nIcon={icon}\n",
        name = name,
        exec = exec,
        icon = icon,
    );
    let path = dir.join(filename);
    fs::write(&path, contents).expect("write should succeed for test fixture .desktop file");
}

#[test]
fn gather_applications_lists_all_desktop_files_in_supplied_dirs() {
    let temp = tempdir().expect("tempdir should be creatable");
    let temp_path = temp.path();

    let data_home_apps = temp_path.join("data_home").join("applications");
    let usr_share_apps = temp_path
        .join("data_dirs")
        .join("usr_share")
        .join("applications");

    write_desktop(
        &data_home_apps,
        "alpha.desktop",
        "Alpha",
        "true",
        "test-icon-alpha",
    );
    write_desktop(
        &data_home_apps,
        "beta.desktop",
        "Beta",
        "true",
        "test-icon-beta",
    );
    write_desktop(
        &usr_share_apps,
        "gamma.desktop",
        "Gamma",
        "true",
        "test-icon-gamma",
    );

    // Non-.desktop file should be ignored.
    fs::write(data_home_apps.join("readme.txt"), "not a desktop file\n")
        .expect("write should succeed for readme.txt");

    // Empty subdir should not be recursed into.
    fs::create_dir_all(data_home_apps.join("subdir"))
        .expect("create_dir_all should succeed for empty subdir");

    // Non-existent path must be silently skipped.
    let nonexistent = temp_path.join("nonexistent").join("applications");

    let dirs: Vec<PathBuf> = vec![data_home_apps.clone(), usr_share_apps.clone(), nonexistent];

    let mut apps: Vec<Application> = gather_applications(&dirs);
    apps.sort_by(|a, b| a.desktop_id.cmp(&b.desktop_id));

    let expected_app_count = 3;
    assert_eq!(
        apps.len(),
        expected_app_count,
        "expected {expected_app_count} apps, got {apps:?}"
    );

    let names: Vec<String> = apps.iter().map(|a| a.name.clone()).collect();
    assert_eq!(
        names,
        vec!["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()],
        "names sorted by desktop_id should be Alpha, Beta, Gamma; got {names:?}"
    );

    let desktop_ids: Vec<String> = apps.iter().map(|a| a.desktop_id.clone()).collect();
    assert_eq!(
        desktop_ids,
        vec![
            "alpha.desktop".to_string(),
            "beta.desktop".to_string(),
            "gamma.desktop".to_string(),
        ],
        "desktop_ids should be canonical .desktop-suffixed names sorted; got {desktop_ids:?}"
    );

    let icons: Vec<Option<String>> = apps.iter().map(|a| a.icon.clone()).collect();
    assert_eq!(
        icons,
        vec![
            Some("test-icon-alpha".to_string()),
            Some("test-icon-beta".to_string()),
            Some("test-icon-gamma".to_string()),
        ],
        "icons sorted by desktop_id should match the fixtures; got {icons:?}"
    );
}

#[test]
fn gather_applications_dedupes_by_desktop_id_first_wins() {
    let temp = tempdir().expect("tempdir should be creatable");
    let temp_path = temp.path();

    let data_home_apps = temp_path.join("data_home").join("applications");
    let usr_share_apps = temp_path
        .join("data_dirs")
        .join("usr_share")
        .join("applications");

    // Same desktop_id in both dirs, with deliberately different Name/Icon so
    // we can tell which copy survived dedup.
    write_desktop(
        &data_home_apps,
        "ghostty.desktop",
        "Ghostty User",
        "true",
        "ghostty-user",
    );
    write_desktop(
        &usr_share_apps,
        "ghostty.desktop",
        "Ghostty System",
        "true",
        "ghostty-system",
    );

    // Unique entry per dir so the test is not trivially satisfied by "only
    // return entries from the first dir" — both dirs must actually be walked.
    write_desktop(
        &data_home_apps,
        "solo-home.desktop",
        "Solo Home",
        "true",
        "test-icon-solo-home",
    );
    write_desktop(
        &usr_share_apps,
        "solo-system.desktop",
        "Solo System",
        "true",
        "test-icon-solo-system",
    );

    // Order matters: data_home_apps first, so its ghostty.desktop should win.
    let dirs: Vec<PathBuf> = vec![data_home_apps.clone(), usr_share_apps.clone()];
    let apps: Vec<Application> = gather_applications(&dirs);

    let expected_app_count = 3;
    assert_eq!(
        apps.len(),
        expected_app_count,
        "expected {expected_app_count} apps after deduping ghostty.desktop; got {apps:?}"
    );

    let ghostty_entries: Vec<&Application> = apps
        .iter()
        .filter(|a| a.desktop_id == "ghostty.desktop")
        .collect();
    assert_eq!(
        ghostty_entries.len(),
        1,
        "expected exactly one ghostty.desktop entry after dedup; got {ghostty_entries:?}"
    );

    let ghostty = ghostty_entries[0];
    assert_eq!(
        ghostty.name, "Ghostty User",
        "first dir (data_home) should win; expected name 'Ghostty User', got {:?}",
        ghostty.name
    );
    assert_eq!(
        ghostty.icon,
        Some("ghostty-user".to_string()),
        "first dir (data_home) should win; expected icon Some(\"ghostty-user\"), got {:?}",
        ghostty.icon
    );

    let mut names: Vec<String> = apps.iter().map(|a| a.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "Ghostty User".to_string(),
            "Solo Home".to_string(),
            "Solo System".to_string(),
        ],
        "sorted names should include the surviving Ghostty entry plus one unique app from each dir; got {names:?}"
    );
}

#[test]
fn gather_applications_follows_symlinks_to_desktop_files() {
    let temp = tempdir().expect("tempdir should be creatable");
    let temp_path = temp.path();

    let targets_dir = temp_path.join("targets");
    let links_dir = temp_path.join("links");
    fs::create_dir_all(&links_dir).expect("create_dir_all should succeed for links dir");

    // Real .desktop file in targets/, written via the existing helper.
    write_desktop(
        &targets_dir,
        "linked.desktop",
        "Linked App",
        "true",
        "test-icon-linked",
    );

    // Live symlink in links/ pointing at the real file (absolute target).
    symlink(
        targets_dir.join("linked.desktop"),
        links_dir.join("linked.desktop"),
    )
    .expect("symlink to live target should succeed");

    // Dangling symlink in the same scanned dir. The target need not exist.
    symlink(
        targets_dir.join("does_not_exist.desktop"),
        links_dir.join("missing.desktop"),
    )
    .expect("symlink to missing target should still succeed");

    // Scan ONLY the links dir. Excluding targets_dir is the key design
    // choice: if the gatherer regresses to `DirEntry::file_type().is_file()`,
    // the live symlink will be dropped and this test fails.
    let dirs: Vec<PathBuf> = vec![links_dir.clone()];
    let apps = gather_applications(&dirs);

    assert_eq!(
        apps.len(),
        1,
        "expected 1 app (live symlink should resolve; dangling symlink should be skipped); got {apps:?}"
    );
    assert_eq!(
        apps[0].name, "Linked App",
        "name should round-trip from symlinked target; got {:?}",
        apps[0].name
    );
}
