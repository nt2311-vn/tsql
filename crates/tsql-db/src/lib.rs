#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseDriver {
    Postgres,
    Sqlite,
}

pub trait DatabaseConnection {
    fn driver(&self) -> DatabaseDriver;
}

#[cfg(test)]
mod tests {
    use super::DatabaseDriver;

    #[test]
    fn postgres_driver_is_distinct_from_sqlite() {
        assert_ne!(DatabaseDriver::Postgres, DatabaseDriver::Sqlite);
    }
}
