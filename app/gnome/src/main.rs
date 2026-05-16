use adw::prelude::*;
use gtk::glib;
use lofi_core::Entry;
use lofi_gnome::{apps, ui};

const APP_ID: &str = "dev.jplein.LoFi";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(on_activate);
    app.run()
}

fn on_activate(app: &adw::Application) {
    let dirs = apps::application_directories();
    let applications = apps::gather_applications(&dirs);
    let entries: Vec<Entry> = applications.into_iter().map(Entry::Application).collect();
    ui::build(app, entries);
}
