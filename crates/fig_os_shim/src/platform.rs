use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum Os {
    Mac,
}

impl Os {
    pub fn all() -> &'static [Self] {
        &[Self::Mac]
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Mac => "macos",
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_os() {
        for os in Os::all() {
            let _ = os.as_str();
            println!("{os:?} {os}");
        }
    }
}
