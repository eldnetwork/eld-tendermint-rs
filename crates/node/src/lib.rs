//! CORS header values the RPC listener sends. The status test reads these same values.

/// `Access-Control-Allow-Origin` on every listener response.
pub const CORS_ALLOW_ORIGIN: &str = "*";
/// `Access-Control-Allow-Methods` on every listener response.
pub const CORS_ALLOW_METHODS: &str = "GET, POST, OPTIONS";
/// `Access-Control-Allow-Headers` on every listener response.
pub const CORS_ALLOW_HEADERS: &str = "Content-Type";
