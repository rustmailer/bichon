//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use crate::{
    modules::{
        error::{code::ErrorCode, BichonResult},
        imap::session::SessionStream,
    },
    raise_error,
};
use rustls::{
    client::danger::{ServerCertVerified, ServerCertVerifier},
    RootCertStore,
};
use std::sync::Arc;

pub async fn establish_tls_stream(
    server_hostname: &str,
    alpn_protocols: &[&str],
    stream: impl SessionStream + 'static,
    dangerous: bool,
) -> BichonResult<impl SessionStream> {
    let tls_stream =
        establish_rustls_stream(server_hostname, alpn_protocols, stream, dangerous).await?;
    let boxed_stream: Box<dyn SessionStream> = Box::new(tls_stream);
    Ok(boxed_stream)
}

async fn establish_rustls_stream(
    server_hostname: &str,
    alpn_protocols: &[&str],
    stream: impl SessionStream,
    dangerous: bool,
) -> BichonResult<impl SessionStream> {
    // Create a root certificate store and add default trusted roots
    let root_store = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.into(),
    };
    let mut config = if dangerous {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipCertVerification))
            .with_no_client_auth()
    } else {
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    };

    // Set the ALPN protocols
    config.alpn_protocols = alpn_protocols
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();

    let tls_connector = tokio_rustls::TlsConnector::from(Arc::new(config));

    let server_name = rustls_pki_types::ServerName::try_from(server_hostname)
        .map_err(|_| raise_error!("Invalid DNS name".into(), ErrorCode::NetworkError))?
        .to_owned();

    let tls_stream = tls_connector
        .connect(server_name, stream)
        .await
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::NetworkError))?;

    Ok(tls_stream)
}

#[derive(Debug)]
struct SkipCertVerification;

impl ServerCertVerifier for SkipCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // Always return a valid certificate verification result
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA1,
            rustls::SignatureScheme::ECDSA_SHA1_Legacy,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}
