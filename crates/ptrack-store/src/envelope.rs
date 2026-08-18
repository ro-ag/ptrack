use crate::EnvelopeError;

const RECORD_MAGIC: [u8; 4] = *b"PTRK";
pub(crate) const RECORD_ENVELOPE_HEADER_LENGTH: usize = 4 + 2 + 2 + 4 + 8;

/// The current binary layout version of a persisted record envelope.
pub const RECORD_ENVELOPE_VERSION: u16 = 1;
/// Stable codec identifier for payloads encoded by Go's `encoding/gob`.
pub const LEGACY_CODEC_GO_GOB: u16 = 1;
/// Stable codec identifier for legacy values stored as uninterpreted bytes.
pub const LEGACY_CODEC_RAW: u16 = 2;
/// Stable codec identifier for canonical native ptrack positional records.
pub const NATIVE_CODEC: u16 = 3;
/// Current payload schema for canonical native ptrack positional records.
pub const NATIVE_PAYLOAD_SCHEMA: u32 = 2;

/// A versioned wrapper around an opaque persisted model payload.
///
/// Codec identifiers are deliberately not interpreted here. This keeps unknown
/// codecs round-trippable so a newer importer can inspect or rewrite them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordEnvelope {
    codec: u16,
    payload_schema: u32,
    payload: Vec<u8>,
}

impl RecordEnvelope {
    /// Wraps arbitrary payload bytes without interpreting the codec or schema.
    pub fn new(codec: u16, payload_schema: u32, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            codec,
            payload_schema,
            payload: payload.into(),
        }
    }

    /// Returns the opaque codec identifier stored in the envelope.
    #[must_use]
    pub const fn codec(&self) -> u16 {
        self.codec
    }

    /// Returns the model payload schema version stored in the envelope.
    #[must_use]
    pub const fn payload_schema(&self) -> u32 {
        self.payload_schema
    }

    /// Returns the exact opaque payload bytes stored in the envelope.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Consumes the envelope and returns its exact opaque payload bytes.
    #[must_use]
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }

    /// Encodes the envelope using the stable big-endian v1 layout.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let payload_length = u64::try_from(self.payload.len())
            .expect("Rust targets cannot address a payload larger than u64::MAX bytes");
        let mut encoded = Vec::with_capacity(RECORD_ENVELOPE_HEADER_LENGTH + self.payload.len());
        encoded.extend_from_slice(&RECORD_MAGIC);
        encoded.extend_from_slice(&RECORD_ENVELOPE_VERSION.to_be_bytes());
        encoded.extend_from_slice(&self.codec.to_be_bytes());
        encoded.extend_from_slice(&self.payload_schema.to_be_bytes());
        encoded.extend_from_slice(&payload_length.to_be_bytes());
        encoded.extend_from_slice(&self.payload);
        encoded
    }

    /// Strictly decodes one complete envelope.
    ///
    /// The decoder rejects truncated headers and payloads, unsupported envelope
    /// layouts, and trailing bytes. Codec identifiers remain opaque.
    pub fn decode(encoded: &[u8]) -> Result<Self, EnvelopeError> {
        if encoded.len() < RECORD_ENVELOPE_HEADER_LENGTH {
            return Err(EnvelopeError::HeaderTooShort {
                actual: encoded.len(),
                minimum: RECORD_ENVELOPE_HEADER_LENGTH,
            });
        }

        let magic: [u8; 4] = encoded[0..4]
            .try_into()
            .expect("the fixed-width header length was checked");
        if magic != RECORD_MAGIC {
            return Err(EnvelopeError::InvalidMagic { actual: magic });
        }

        let envelope_version = u16::from_be_bytes(
            encoded[4..6]
                .try_into()
                .expect("the fixed-width header length was checked"),
        );
        if envelope_version != RECORD_ENVELOPE_VERSION {
            return Err(EnvelopeError::UnsupportedEnvelopeVersion {
                actual: envelope_version,
                supported: RECORD_ENVELOPE_VERSION,
            });
        }

        let codec = u16::from_be_bytes(
            encoded[6..8]
                .try_into()
                .expect("the fixed-width header length was checked"),
        );
        let payload_schema = u32::from_be_bytes(
            encoded[8..12]
                .try_into()
                .expect("the fixed-width header length was checked"),
        );
        let declared_length = u64::from_be_bytes(
            encoded[12..20]
                .try_into()
                .expect("the fixed-width header length was checked"),
        );
        let payload_length =
            usize::try_from(declared_length).map_err(|_| EnvelopeError::PayloadLengthOverflow {
                declared: declared_length,
            })?;
        let expected_length = RECORD_ENVELOPE_HEADER_LENGTH
            .checked_add(payload_length)
            .ok_or(EnvelopeError::PayloadLengthOverflow {
                declared: declared_length,
            })?;

        match encoded.len().cmp(&expected_length) {
            std::cmp::Ordering::Less => Err(EnvelopeError::PayloadTooShort {
                declared: declared_length,
                actual: encoded.len() - RECORD_ENVELOPE_HEADER_LENGTH,
            }),
            std::cmp::Ordering::Greater => Err(EnvelopeError::TrailingBytes {
                declared: declared_length,
                trailing: encoded.len() - expected_length,
            }),
            std::cmp::Ordering::Equal => Ok(Self {
                codec,
                payload_schema,
                payload: encoded[RECORD_ENVELOPE_HEADER_LENGTH..].to_vec(),
            }),
        }
    }
}
