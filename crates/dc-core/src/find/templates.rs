//! Saved searches (the "Load/Save" page of the original, its search
//! templates): kept in one TOML file in the user's configuration
//! directory, `~/.config/doubycmd/searches.toml`, readable and editable by
//! hand. A search saved without its start directory runs where the panel is.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use dc_i18n::{fill, gettext};
use serde::{Deserialize, Serialize};

use super::FindQuery;

/// One saved search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub query: FindQuery,
}

/// All saved searches, in the order saved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Templates {
    #[serde(default, rename = "search")]
    pub list: Vec<Template>,
}

/// The file of saved searches cannot be read or written.
#[derive(Debug)]
pub struct TemplatesError {
    pub path: PathBuf,
    pub reason: String,
}

impl fmt::Display for TemplatesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&fill(&gettext("Saved searches in {path} cannot be used"), &[("path", &self.path.display())]))
    }
}

impl std::error::Error for TemplatesError {}

impl Templates {
    /// The searches saved in `path`, none when there is no such file.
    ///
    /// # Errors
    /// When the file is there but cannot be read or understood.
    pub fn load(path: &Path) -> Result<Self, TemplatesError> {
        let error = |reason: String| TemplatesError { path: path.to_owned(), reason };
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|err| error(err.to_string())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(error(err.to_string())),
        }
    }

    /// Writes all to `path` (its directory created when missing), whole or
    /// not at all: a new file renamed over the old one.
    ///
    /// # Errors
    /// When the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), TemplatesError> {
        let error = |reason: String| TemplatesError { path: path.to_owned(), reason };
        let text = toml::to_string_pretty(self).map_err(|err| error(err.to_string()))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|err| error(err.to_string()))?;
        }
        let new = path.with_extension("toml.new");
        fs::write(&new, text).and_then(|()| fs::rename(&new, path)).map_err(|err| error(err.to_string()))
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Template> {
        self.list.iter().find(|template| template.name == name)
    }

    /// Saves `template`, in place of one of the same name.
    pub fn set(&mut self, template: Template) {
        match self.list.iter_mut().find(|earlier| earlier.name == template.name) {
            Some(earlier) => *earlier = template,
            None => self.list.push(template),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.list.retain(|template| template.name != name);
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::super::{Age, Duplicates, TimeUnit};
    use super::*;

    #[test]
    fn saved_and_loaded_again() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("doubycmd/searches.toml");
        assert_eq!(Templates::load(&path).unwrap(), Templates::default());

        let mut templates = Templates::default();
        let mut query = FindQuery { masks: "*.rs".to_owned(), depth: Some(2), ..FindQuery::default() };
        query.text.enabled = true;
        query.text.text = "fn main".to_owned();
        query.date_from = Some(date(2026, 1, 31));
        query.age = Some(Age { amount: 3, unit: TimeUnit::Week });
        query.duplicates = Some(Duplicates { size: true, ..Duplicates::default() });
        templates.set(Template { name: "Rust".to_owned(), query: query.clone() });
        templates.set(Template { name: "Vše".to_owned(), query: FindQuery::default() });
        templates.save(&path).unwrap();
        let loaded = Templates::load(&path).unwrap();
        assert_eq!(loaded, templates);
        assert_eq!(loaded.get("Rust").map(|template| &template.query), Some(&query));

        templates.set(Template { name: "Rust".to_owned(), query: FindQuery::default() });
        templates.remove("Vše");
        assert_eq!(templates.list.len(), 1);
        assert_eq!(templates.list[0].query, FindQuery::default());

        fs::write(&path, "search = 5").unwrap();
        assert!(Templates::load(&path).is_err());
    }
}
