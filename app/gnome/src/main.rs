use adw::prelude::*;
use gtk::glib;
use lofi_core::Entry;
use lofi_gnome::{apps, ui, windows};

const APP_ID: &str = "dev.jplein.LoFi";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(on_activate);
    app.run()
}

fn on_activate(app: &adw::Application) {
    let dirs = apps::application_directories();
    let applications = apps::gather_applications(&dirs);
    let windows = windows::gather_windows();
    let mut entries: Vec<Entry> = Vec::with_capacity(applications.len() + windows.len());
    entries.extend(applications.into_iter().map(Entry::Application));
    entries.extend(windows.into_iter().map(Entry::Window));
    ui::build(app, entries);
}
