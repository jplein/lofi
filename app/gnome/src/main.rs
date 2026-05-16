use adw::prelude::*;
use gtk::glib;

const APP_ID: &str = "dev.jplein.LoFi";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    let label = gtk::Label::new(Some("Hello, world"));
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("LoFi")
        .default_width(400)
        .default_height(120)
        .content(&label)
        .build();
    window.present();
}
