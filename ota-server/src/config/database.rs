//! SQLite for local development, PostgreSQL for deployment. No silent failover.
use crate::errors::{ApiError, ApiResult};
use std::path::Path;

pub fn resolve_database_url(
    data_dir: &Path,
    backend: Option<&str>,
    value: Option<&str>,
) -> ApiResult<String> {
    let backend = backend
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_ascii_lowercase);
    let value = value.map(str::trim).filter(|v| !v.is_empty());
    if backend
        .as_deref()
        .is_some_and(|v| !matches!(v, "sqlite" | "postgres" | "postgresql"))
    {
        return Err(invalid("DB_BACKEND must be sqlite or postgres"));
    }
    let Some(value) = value else {
        if backend.as_deref().is_some_and(|v| v != "sqlite") {
            return Err(invalid("DATABASE_URL is required for DB_BACKEND=postgres"));
        }
        let directory = data_dir
            .to_str()
            .ok_or_else(|| invalid("OTA_DATA_DIR must be valid UTF-8"))?
            .trim_end_matches(['/', '\\'])
            .replace("\\\\?\\", "")
            .replace('\\', "/");
        if directory.contains(['?', '#']) {
            return Err(invalid(
                "OTA_DATA_DIR cannot contain URL query/fragment delimiters; set DATABASE_URL explicitly",
            ));
        }
        return Ok(format!("sqlite:{directory}/ota.db?mode=rwc"));
    };
    let scheme = value.split_once(':').map(|v| v.0);
    if !matches!(scheme, Some("sqlite" | "postgres" | "postgresql")) {
        return Err(invalid(
            "DATABASE_URL must use sqlite:, postgres: or postgresql:",
        ));
    }
    if backend
        .as_deref()
        .is_some_and(|v| (v == "sqlite") != (scheme == Some("sqlite")))
    {
        return Err(invalid(
            "DB_BACKEND conflicts with DATABASE_URL; select one database per process",
        ));
    }
    if scheme == Some("sqlite") {
        return Ok(value.to_owned());
    }
    // Never echo parser errors/URLs, including unknown options which SQLx may log.
    let parsed =
        url::Url::parse(value).map_err(|_| invalid("Malformed PostgreSQL DATABASE_URL"))?;
    let host = parsed
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| invalid("PostgreSQL DATABASE_URL needs a hostname"))?;
    if parsed.path().trim_matches('/').is_empty() || parsed.fragment().is_some() {
        return Err(invalid(
            "PostgreSQL DATABASE_URL needs a database name and no fragment; URL-encode credentials",
        ));
    }
    let mut sslmode = None;
    let mut seen = std::collections::HashSet::new();
    for (key, val) in parsed.query_pairs() {
        if !matches!(
            key.as_ref(),
            "sslmode"
                | "ssl-mode"
                | "sslrootcert"
                | "ssl-root-cert"
                | "sslcert"
                | "ssl-cert"
                | "sslkey"
                | "ssl-key"
                | "application_name"
                | "statement-cache-capacity"
        ) {
            return Err(invalid(
                "Unsupported DATABASE_URL option; use documented SQLx PostgreSQL TLS/connection options",
            ));
        }
        if !seen.insert(key.replace('-', "")) {
            return Err(invalid("Duplicate DATABASE_URL option"));
        }
        if matches!(key.as_ref(), "sslmode" | "ssl-mode") {
            sslmode = Some(val.into_owned());
        }
    }
    let azure = host
        .to_ascii_lowercase()
        .trim_end_matches('.')
        .ends_with(".postgres.database.azure.com");
    if azure && sslmode.as_deref() != Some("verify-full") {
        return Err(invalid(
            "Azure PostgreSQL requires sslmode=verify-full; configure a trusted CA, never disable TLS verification",
        ));
    }
    Ok(value.to_owned())
}

fn invalid(message: &'static str) -> ApiError {
    ApiError::internal("INVALID_CONFIG", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn resolve(backend: Option<&str>, url: Option<&str>) -> ApiResult<String> {
        resolve_database_url(Path::new("/srv/data"), backend, url)
    }
    #[test]
    fn sqlite_default_paths_and_explicit_urls() {
        assert_eq!(
            resolve(None, None).unwrap(),
            "sqlite:/srv/data/ota.db?mode=rwc"
        );
        assert_eq!(
            resolve(Some(" SQLite "), Some("")).unwrap(),
            "sqlite:/srv/data/ota.db?mode=rwc"
        );
        assert_eq!(
            resolve(None, Some("sqlite:custom.db?mode=rwc")).unwrap(),
            "sqlite:custom.db?mode=rwc"
        );
        assert_eq!(
            resolve_database_url(Path::new(r"\\?\D:\project\data\"), None, None).unwrap(),
            "sqlite:D:/project/data/ota.db?mode=rwc"
        );
        assert!(resolve_database_url(Path::new("/srv/data?other"), None, None).is_err());
    }
    #[test]
    fn postgres_selection_is_explicit_or_backwards_compatible() {
        let url = "postgresql://ota:test@localhost:5432/ota_test";
        assert_eq!(resolve(Some("postgres"), Some(url)).unwrap(), url);
        assert_eq!(resolve(None, Some(url)).unwrap(), url);
        assert!(resolve(Some("sqlite"), Some(url)).is_err());
        assert!(resolve(Some("postgres"), Some("sqlite:test.db")).is_err());
        assert!(resolve(Some("postgres"), None).is_err());
        assert!(resolve(Some("mysql"), None).is_err());
        assert!(resolve(None, Some("https://server/db")).is_err());
    }
    #[test]
    fn azure_tls_options_and_secret_safe_errors() {
        let base = "postgresql://ota:secret@server.postgres.database.azure.com/ota";
        assert!(resolve(None, Some(&format!("{base}?sslmode=verify-full"))).is_ok());
        for query in [
            "",
            "?sslmode=disable",
            "?sslmode=require",
            "?sslmode=verify-full&ssl-mode=disable",
            "?sslmode=verify-full&host=evil",
            "?password=secret",
            "#secret",
        ] {
            let error = resolve(None, Some(&format!("{base}{query}"))).unwrap_err();
            assert!(!format!("{error:?}").contains("secret"));
        }
    }
}
