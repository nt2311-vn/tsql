#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
}

impl Theme {
    #[must_use]
    pub const fn catppuccin_mocha() -> Self {
        Self {
            name: "catppuccin-mocha",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Theme;

    #[test]
    fn default_dark_theme_is_catppuccin_mocha() {
        let theme = Theme::catppuccin_mocha();

        assert_eq!(theme.name, "catppuccin-mocha");
    }
}
