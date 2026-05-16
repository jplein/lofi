#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub name: String,
    pub desktop_id: String,
    pub icon: Option<String>,
}
