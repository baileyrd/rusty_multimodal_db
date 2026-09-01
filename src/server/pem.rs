//! A tiny PEM decoder — exactly what [`super::TlsConfig::from_pem_files`]
//! needs (base64-decode the body between `-----BEGIN ...-----`/`-----END
//! ...-----` markers), not a general-purpose PEM/DER parsing library.
//! `rusty_tls::TlsAcceptor::new` takes DER bytes directly and
//! deliberately doesn't re-expose a PEM parser of its own — see
//! `docs/design/SERVER-TLS-DESIGN.md`'s "Ecosystem check" for why this
//! crate doesn't take a new dependency for this instead: standard base64
//! decoding is a small, fully-specified, deterministic transform with no
//! invisible-to-testing correctness property (unlike, say, constant-time
//! comparison — see `AuthConfig::check`'s own comment on why *that*
//! comparison is not hand-rolled), so hand-rolling it here is a
//! reasonable, well-tested bounded utility.

use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum PemError {
    /// A body between BEGIN/END markers isn't valid base64 (wrong
    /// length, or a character outside the base64 alphabet/padding).
    InvalidBase64,
    /// An `-----END ...-----` marker with no matching `-----BEGIN
    /// ...-----` before it.
    UnmatchedEnd,
    /// A `-----BEGIN ...-----` marker with no matching `-----END
    /// ...-----` before the file (or another `BEGIN`) ends it.
    UnterminatedBlock,
    /// The input had no `-----BEGIN ...-----`/`-----END ...-----` block
    /// at all.
    NoBlocksFound,
}

impl fmt::Display for PemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PemError::InvalidBase64 => write!(f, "PEM block body is not valid base64"),
            PemError::UnmatchedEnd => write!(f, "PEM END marker with no matching BEGIN"),
            PemError::UnterminatedBlock => write!(f, "PEM BEGIN marker with no matching END"),
            PemError::NoBlocksFound => write!(f, "no PEM BEGIN/END block found"),
        }
    }
}

impl std::error::Error for PemError {}

/// Decode every `-----BEGIN ...-----`/`-----END ...-----` block in `pem`
/// into its raw DER bytes, in the order they appear. The label between
/// `BEGIN`/`END` (`CERTIFICATE`, `PRIVATE KEY`, `RSA PRIVATE KEY`, ...)
/// is not checked — `rusty_tls::TlsAcceptor::new` auto-detects the DER
/// key format itself, so this decoder stays label-agnostic.
pub fn decode_blocks(pem: &str) -> Result<Vec<Vec<u8>>, PemError> {
    let mut blocks = Vec::new();
    let mut body = String::new();
    let mut in_block = false;
    for line in pem.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("-----BEGIN ") && trimmed.ends_with("-----") {
            if in_block {
                return Err(PemError::UnterminatedBlock);
            }
            in_block = true;
            body.clear();
            continue;
        }
        if trimmed.starts_with("-----END ") && trimmed.ends_with("-----") {
            if !in_block {
                return Err(PemError::UnmatchedEnd);
            }
            blocks.push(base64_decode(&body)?);
            in_block = false;
            continue;
        }
        if in_block {
            body.push_str(trimmed);
        }
    }
    if in_block {
        return Err(PemError::UnterminatedBlock);
    }
    if blocks.is_empty() {
        return Err(PemError::NoBlocksFound);
    }
    Ok(blocks)
}

/// Standard base64 (RFC 4648 §4) decode, tolerant of embedded
/// whitespace/newlines (PEM wraps its base64 body at a fixed line
/// length) — the only variant PEM ever uses.
fn base64_decode(input: &str) -> Result<Vec<u8>, PemError> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let cleaned: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if cleaned.is_empty() || cleaned.len() % 4 != 0 {
        return Err(PemError::InvalidBase64);
    }
    let pad = cleaned.iter().rev().take_while(|&&b| b == b'=').count();
    if pad > 2 || cleaned[..cleaned.len() - pad].contains(&b'=') {
        return Err(PemError::InvalidBase64);
    }

    let mut out = Vec::with_capacity(cleaned.len() / 4 * 3);
    for chunk in cleaned.chunks_exact(4) {
        let mut vals = [0u8; 4];
        for (slot, &byte) in vals.iter_mut().zip(chunk) {
            *slot = if byte == b'=' {
                0
            } else {
                value(byte).ok_or(PemError::InvalidBase64)?
            };
        }
        out.push((vals[0] << 2) | (vals[1] >> 4));
        out.push((vals[1] << 4) | (vals[2] >> 2));
        out.push((vals[2] << 6) | vals[3]);
    }
    out.truncate(out.len() - pad);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decode_matches_a_known_vector() {
        assert_eq!(base64_decode("SGVsbG8=").unwrap(), b"Hello");
        assert_eq!(base64_decode("SGVsbG8gd29ybGQ=").unwrap(), b"Hello world");
        assert_eq!(base64_decode("AAAA").unwrap(), vec![0u8, 0, 0]);
    }

    #[test]
    fn base64_decode_ignores_embedded_whitespace() {
        assert_eq!(base64_decode("SGVs\nbG8=").unwrap(), b"Hello");
        assert_eq!(base64_decode(" SGVsbG8=\n").unwrap(), b"Hello");
    }

    #[test]
    fn base64_decode_rejects_malformed_input() {
        assert_eq!(base64_decode(""), Err(PemError::InvalidBase64));
        assert_eq!(base64_decode("SGVsbG8"), Err(PemError::InvalidBase64)); // wrong length
        assert_eq!(base64_decode("SGVsb!8="), Err(PemError::InvalidBase64)); // bad char
        assert_eq!(base64_decode("S=VsbG8="), Err(PemError::InvalidBase64)); // '=' mid-string
    }

    #[test]
    fn decode_blocks_finds_a_single_certificate_block() {
        let pem = "-----BEGIN CERTIFICATE-----\nSGVsbG8=\n-----END CERTIFICATE-----\n";
        assert_eq!(decode_blocks(pem).unwrap(), vec![b"Hello".to_vec()]);
    }

    #[test]
    fn decode_blocks_finds_every_block_in_a_chain_leaf_first() {
        let pem = "-----BEGIN CERTIFICATE-----\n\
                   SGVsbG8=\n\
                   -----END CERTIFICATE-----\n\
                   -----BEGIN CERTIFICATE-----\n\
                   d29ybGQ=\n\
                   -----END CERTIFICATE-----\n";
        assert_eq!(
            decode_blocks(pem).unwrap(),
            vec![b"Hello".to_vec(), b"world".to_vec()]
        );
    }

    #[test]
    fn decode_blocks_is_label_agnostic() {
        // A private key's label varies (PRIVATE KEY / RSA PRIVATE KEY / EC
        // PRIVATE KEY) — rusty_tls::TlsAcceptor::new auto-detects the real
        // format from the DER bytes themselves, so this decoder doesn't
        // need to distinguish them either.
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nSGVsbG8=\n-----END RSA PRIVATE KEY-----\n";
        assert_eq!(decode_blocks(pem).unwrap(), vec![b"Hello".to_vec()]);
    }

    #[test]
    fn decode_blocks_rejects_an_unmatched_end() {
        let pem = "-----END CERTIFICATE-----\n";
        assert_eq!(decode_blocks(pem), Err(PemError::UnmatchedEnd));
    }

    #[test]
    fn decode_blocks_rejects_an_unterminated_block() {
        let pem = "-----BEGIN CERTIFICATE-----\nSGVsbG8=\n";
        assert_eq!(decode_blocks(pem), Err(PemError::UnterminatedBlock));
    }

    #[test]
    fn decode_blocks_rejects_input_with_no_blocks() {
        assert_eq!(
            decode_blocks("not a PEM file at all"),
            Err(PemError::NoBlocksFound)
        );
    }

    #[test]
    fn decode_blocks_propagates_a_malformed_body() {
        let pem = "-----BEGIN CERTIFICATE-----\nnot-base64!!\n-----END CERTIFICATE-----\n";
        assert_eq!(decode_blocks(pem), Err(PemError::InvalidBase64));
    }
}
