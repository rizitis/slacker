//! One application offered by a remote, as the install window needs it.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub name: String,
    pub description: String,
    pub id: String,
    pub version: String,
    pub branch: String,
    /// Remotes that offer it; flatpak joins several with commas.
    pub remotes: Vec<String>,
}

impl Hit {
    pub fn title(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}
