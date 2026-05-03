#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub name: &'static str,
    pub version: &'static str,
}

impl Default for ProjectInfo {
    fn default() -> Self {
        Self {
            name: "tsql",
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectInfo;

    #[test]
    fn default_project_info_has_name() {
        let info = ProjectInfo::default();

        assert_eq!(info.name, "tsql");
    }
}
