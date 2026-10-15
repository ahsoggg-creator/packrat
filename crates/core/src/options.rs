//! Options that change how an archive is opened.

use crate::encoding::NameEncoding;
use std::fmt;
use std::sync::Arc;

/// A password. It never appears in `Debug` output, error messages or logs.
#[derive(Clone, PartialEq, Eq)]
pub struct Password(Arc<str>);

impl Password {
    pub fn new(text: &str) -> Self {
        Password(Arc::from(text))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(***)")
    }
}

#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    /// Code page for ZIP names that carry no encoding information; `None` = detect.
    pub encoding: Option<NameEncoding>,
    pub password: Option<Password>,
}

impl OpenOptions {
    pub fn with_encoding(mut self, e: NameEncoding) -> Self {
        self.encoding = Some(e);
        self
    }

    pub fn with_password(mut self, p: &str) -> Self {
        self.password = Some(Password::new(p));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_is_never_printed() {
        let o = OpenOptions::default().with_password("hunter2");
        assert!(!format!("{o:?}").contains("hunter2"));
        assert_eq!(o.password.unwrap().expose(), "hunter2");
    }
}
