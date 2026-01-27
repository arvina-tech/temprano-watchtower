const GROUP_NONCE_MAGIC: [u8; 4] = *b"NKG1";
const GROUP_NONCE_VERSION: u8 = 0x01;
const GROUP_NONCE_FLAG_MASK: u16 = 0x003F;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonceKeyEncoding {
    Numeric,
    Ascii,
}

impl NonceKeyEncoding {
    pub fn as_str(&self) -> &'static str {
        match self {
            NonceKeyEncoding::Numeric => "numeric",
            NonceKeyEncoding::Ascii => "ascii",
        }
    }
}

impl TryFrom<u16> for NonceKeyEncoding {
    type Error = ();

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(NonceKeyEncoding::Numeric),
            1 => Ok(NonceKeyEncoding::Ascii),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedNonceKeyField {
    pub encoding: NonceKeyEncoding,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedNonceKey {
    pub kind: u8,
    pub scope: DecodedNonceKeyField,
    pub group: DecodedNonceKeyField,
    pub memo: DecodedNonceKeyField,
}

pub fn is_group_nonce_key(bytes: &[u8]) -> bool {
    if bytes.len() != 32 {
        return false;
    }
    if bytes[..4] != GROUP_NONCE_MAGIC {
        return false;
    }
    if bytes[4] != GROUP_NONCE_VERSION {
        return false;
    }
    let flags = u16::from_be_bytes([bytes[6], bytes[7]]);
    if flags & !GROUP_NONCE_FLAG_MASK != 0 {
        return false;
    }
    let scope_encoding = flags & 0b11;
    let group_encoding = (flags >> 2) & 0b11;
    let memo_encoding = (flags >> 4) & 0b11;
    if scope_encoding > 1 || group_encoding > 1 || memo_encoding > 1 {
        return false;
    }
    if scope_encoding == 1 && !is_ascii_field(&bytes[8..16]) {
        return false;
    }
    if group_encoding == 1 && !is_ascii_field(&bytes[16..20]) {
        return false;
    }
    if memo_encoding == 1 && !is_ascii_field(&bytes[20..32]) {
        return false;
    }
    true
}

pub fn decode_group_nonce_key(bytes: &[u8]) -> Option<DecodedNonceKey> {
    if !is_group_nonce_key(bytes) {
        return None;
    }

    let flags = u16::from_be_bytes([bytes[6], bytes[7]]);
    let scope_encoding = NonceKeyEncoding::try_from(flags & 0b11).ok()?;
    let group_encoding = NonceKeyEncoding::try_from((flags >> 2) & 0b11).ok()?;
    let memo_encoding = NonceKeyEncoding::try_from((flags >> 4) & 0b11).ok()?;

    let scope = decode_field(&bytes[8..16], scope_encoding, FieldKind::Scope);
    let group = decode_field(&bytes[16..20], group_encoding, FieldKind::Group);
    let memo = decode_field(&bytes[20..32], memo_encoding, FieldKind::Memo);

    Some(DecodedNonceKey {
        kind: bytes[5],
        scope,
        group,
        memo,
    })
}

#[derive(Debug, Clone, Copy)]
enum FieldKind {
    Scope,
    Group,
    Memo,
}

fn decode_field(bytes: &[u8], encoding: NonceKeyEncoding, kind: FieldKind) -> DecodedNonceKeyField {
    let value = match encoding {
        NonceKeyEncoding::Numeric => decode_numeric(bytes, kind),
        NonceKeyEncoding::Ascii => decode_ascii(bytes),
    };
    DecodedNonceKeyField { encoding, value }
}

fn decode_numeric(bytes: &[u8], kind: FieldKind) -> String {
    match kind {
        FieldKind::Scope => {
            let value = u64::from_be_bytes(bytes.try_into().unwrap_or([0u8; 8]));
            value.to_string()
        }
        FieldKind::Group => {
            let value = u32::from_be_bytes(bytes.try_into().unwrap_or([0u8; 4]));
            value.to_string()
        }
        FieldKind::Memo => format!("0x{}", hex::encode(bytes)),
    }
}

fn decode_ascii(bytes: &[u8]) -> String {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == 0 {
        end -= 1;
    }
    if bytes[..end]
        .iter()
        .any(|byte| !matches!(*byte, 0x20..=0x7E))
    {
        return format!("0x{}", hex::encode(bytes));
    }
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

fn is_ascii_field(bytes: &[u8]) -> bool {
    let mut zero_seen = false;
    for &byte in bytes {
        if byte == 0 {
            zero_seen = true;
            continue;
        }
        if zero_seen {
            return false;
        }
        if !(0x20..=0x7E).contains(&byte) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        DecodedNonceKey, GROUP_NONCE_MAGIC, GROUP_NONCE_VERSION, decode_group_nonce_key,
        is_group_nonce_key,
    };

    fn build_key(flags: u16, scope: [u8; 8], group: [u8; 4], memo: [u8; 12]) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        bytes[..4].copy_from_slice(&GROUP_NONCE_MAGIC);
        bytes[4] = GROUP_NONCE_VERSION;
        bytes[6..8].copy_from_slice(&flags.to_be_bytes());
        bytes[8..16].copy_from_slice(&scope);
        bytes[16..20].copy_from_slice(&group);
        bytes[20..32].copy_from_slice(&memo);
        bytes
    }

    fn padded_ascii<const N: usize>(value: &str) -> [u8; N] {
        let bytes = value.as_bytes();
        assert!(bytes.len() <= N);
        let mut out = [0u8; N];
        out[..bytes.len()].copy_from_slice(bytes);
        out
    }

    #[test]
    fn accepts_numeric_format() {
        let key = build_key(0, [0u8; 8], [0u8; 4], [0u8; 12]);
        assert!(is_group_nonce_key(&key));
    }

    #[test]
    fn accepts_ascii_format() {
        let flags = 0b01 | (0b01 << 2) | (0b01 << 4);
        let key = build_key(
            flags,
            padded_ascii("PAYROLL"),
            padded_ascii("G1"),
            padded_ascii("JAN-2026"),
        );
        assert!(is_group_nonce_key(&key));
    }

    #[test]
    fn rejects_wrong_length() {
        let bytes = [0u8; 31];
        assert!(!is_group_nonce_key(&bytes));
    }

    #[test]
    fn rejects_wrong_magic() {
        let mut key = build_key(0, [0u8; 8], [0u8; 4], [0u8; 12]);
        key[0] = 0x00;
        assert!(!is_group_nonce_key(&key));
    }

    #[test]
    fn rejects_wrong_version() {
        let mut key = build_key(0, [0u8; 8], [0u8; 4], [0u8; 12]);
        key[4] = 0x02;
        assert!(!is_group_nonce_key(&key));
    }

    #[test]
    fn rejects_reserved_bits() {
        let key = build_key(0x0040, [0u8; 8], [0u8; 4], [0u8; 12]);
        assert!(!is_group_nonce_key(&key));
    }

    #[test]
    fn rejects_reserved_encodings() {
        let flags = 0b10 | (0b11 << 2) | (0b10 << 4);
        let key = build_key(flags, [0u8; 8], [0u8; 4], [0u8; 12]);
        assert!(!is_group_nonce_key(&key));
    }

    #[test]
    fn rejects_non_printable_ascii() {
        let flags = 0b01 | (0b01 << 2) | (0b01 << 4);
        let mut memo = [0u8; 12];
        memo[0] = b'H';
        memo[1] = 0x19;
        let key = build_key(flags, padded_ascii("SCOPE"), padded_ascii("G1"), memo);
        assert!(!is_group_nonce_key(&key));
    }

    #[test]
    fn rejects_ascii_with_embedded_zero() {
        let flags = 0b01;
        let mut scope = [0u8; 8];
        scope[0] = b'A';
        scope[1] = 0;
        scope[2] = b'B';
        let key = build_key(flags, scope, [0u8; 4], [0u8; 12]);
        assert!(!is_group_nonce_key(&key));
    }

    #[test]
    fn decodes_numeric_fields() {
        let mut key = build_key(0, [0u8; 8], [0u8; 4], [0u8; 12]);
        key[5] = 0x02;
        key[8..16].copy_from_slice(&1u64.to_be_bytes());
        key[16..20].copy_from_slice(&42u32.to_be_bytes());
        key[20..32].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);

        let decoded = decode_group_nonce_key(&key).expect("decoded");
        assert_eq!(
            decoded,
            DecodedNonceKey {
                kind: 0x02,
                scope: super::DecodedNonceKeyField {
                    encoding: super::NonceKeyEncoding::Numeric,
                    value: "1".to_string(),
                },
                group: super::DecodedNonceKeyField {
                    encoding: super::NonceKeyEncoding::Numeric,
                    value: "42".to_string(),
                },
                memo: super::DecodedNonceKeyField {
                    encoding: super::NonceKeyEncoding::Numeric,
                    value: "0x0102030405060708090a0b0c".to_string(),
                },
            }
        );
    }

    #[test]
    fn decodes_ascii_fields() {
        let flags = 0b01 | (0b01 << 2) | (0b01 << 4);
        let key = build_key(
            flags,
            padded_ascii("PAYROLL"),
            padded_ascii("G1"),
            padded_ascii("JAN-2026"),
        );
        let decoded = decode_group_nonce_key(&key).expect("decoded");
        assert_eq!(decoded.kind, 0x00);
        assert_eq!(decoded.scope.value, "PAYROLL");
        assert_eq!(decoded.group.value, "G1");
        assert_eq!(decoded.memo.value, "JAN-2026");
        assert_eq!(decoded.scope.encoding, super::NonceKeyEncoding::Ascii);
        assert_eq!(decoded.group.encoding, super::NonceKeyEncoding::Ascii);
        assert_eq!(decoded.memo.encoding, super::NonceKeyEncoding::Ascii);
    }
}
