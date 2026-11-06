//! Which TLS connector the service uses: `NoTls`, or rustls with the `tls` feature (Chapter 12).
//!
//! Whether TLS is *required* is the connection string's business (`sslmode=require`); the
//! connector decides whether the server's certificate is *trusted*. tokio-postgres has no
//! `sslmode=verify-full`: verification is always the connector's job, and rustls always
//! verifies the certificate chain and the host name.

use crate::config::DbArgs;

#[cfg(not(feature = "tls"))]
pub type MakeTls = tokio_postgres::NoTls;

#[cfg(feature = "tls")]
pub type MakeTls = tokio_postgres_rustls::MakeRustlsConnect;

/// The connector the settings ask for.
pub fn make_tls(args: &DbArgs) -> anyhow::Result<MakeTls> {
    #[cfg(feature = "tls")]
    return connector(args.db_ca_file.as_deref());
    #[cfg(not(feature = "tls"))]
    {
        let _ = args;
        connector(None)
    }
}

/// The connector, trusting `ca_file` (a PEM file) or, without one, the system's roots.
/// Without the `tls` feature, `NoTls`.
#[cfg(not(feature = "tls"))]
pub fn connector(_ca_file: Option<&std::path::Path>) -> anyhow::Result<MakeTls> {
    Ok(tokio_postgres::NoTls)
}

#[cfg(feature = "tls")]
pub fn connector(ca_file: Option<&std::path::Path>) -> anyhow::Result<MakeTls> {
    use anyhow::Context;
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::pem::PemObject;

    let mut roots = rustls::RootCertStore::empty();
    match ca_file {
        Some(path) => {
            for cert in CertificateDer::pem_file_iter(path)
                .with_context(|| format!("reading {}", path.display()))?
            {
                roots.add(cert.with_context(|| format!("parsing {}", path.display()))?)?;
            }
        }
        None => {
            let found = rustls_native_certs::load_native_certs();
            for cert in found.certs {
                // A store can hold certificates rustls cannot use; skip those.
                let _ = roots.add(cert);
            }
        }
    }
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}
