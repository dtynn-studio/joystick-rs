pub mod driver;

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;
