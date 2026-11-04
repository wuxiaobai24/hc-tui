//! hc framed 协议编解码。
//!
//! 线格式：`[u32 LE channel][u8 kind][u32 LE len][payload]`，HEADER = 9。
//! 依据：docs/protocol-facts.md §2。

pub const HEADER: usize = 9;
pub const MAX_PAYLOAD: usize = 16 << 20;

pub const DATA: u8 = 0;
pub const CREDIT: u8 = 1;
pub const CLOSE: u8 = 2;
pub const RESIZE: u8 = 3;
pub const FOCUS: u8 = 4;
pub const FACTS: u8 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub channel: u32,
    pub kind: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodecError {
    #[error("frame payload of {0} bytes exceeds {MAX_PAYLOAD}")]
    TooLarge(usize),
}

pub fn encode(channel: u32, kind: u8, payload: &[u8]) -> Result<Vec<u8>, CodecError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(CodecError::TooLarge(payload.len()));
    }
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&channel.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// 只解析头部，payload 由调用方按返回长度读取。
pub fn decode_header(header: &[u8; HEADER]) -> Result<(u32, u8, usize), CodecError> {
    let channel = u32::from_le_bytes(header[0..4].try_into().expect("4 bytes"));
    let kind = header[4];
    let len = u32::from_le_bytes(header[5..9].try_into().expect("4 bytes")) as usize;
    if len > MAX_PAYLOAD {
        return Err(CodecError::TooLarge(len));
    }
    Ok((channel, kind, len))
}

/// 已消费的字节数。**每条流都要回**，包括 events channel。
pub fn credit(channel: u32, bytes: usize) -> Result<Vec<u8>, CodecError> {
    encode(channel, CREDIT, &(bytes as u32).to_le_bytes())
}

/// viewer 内容区尺寸：cols 在前，rows 在后，各 u16 LE。
pub fn resize(channel: u32, cols: u16, rows: u16) -> Result<Vec<u8>, CodecError> {
    let mut payload = Vec::with_capacity(4);
    payload.extend_from_slice(&cols.to_le_bytes());
    payload.extend_from_slice(&rows.to_le_bytes());
    encode(channel, RESIZE, &payload)
}

pub fn focus(channel: u32, focused: bool) -> Result<Vec<u8>, CodecError> {
    encode(channel, FOCUS, &[u8::from(focused)])
}

/// 本地 detach：只断这条流，core 里的程序继续跑。payload 为空。
pub fn close(channel: u32) -> Result<Vec<u8>, CodecError> {
    encode(channel, CLOSE, &[])
}

/// viewer 自身属性。payload 是 JSON `Display{fg,bg,cursor,cell}`。
pub fn facts(channel: u32, display_json: &[u8]) -> Result<Vec<u8>, CodecError> {
    encode(channel, FACTS, display_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let bytes = encode(7, DATA, b"hi").unwrap();
        assert_eq!(bytes[0..4], 7u32.to_le_bytes());
        assert_eq!(bytes[4], DATA);
        assert_eq!(bytes[5..9], 2u32.to_le_bytes());

        let header: [u8; HEADER] = bytes[..HEADER].try_into().unwrap();
        let (channel, kind, len) = decode_header(&header).unwrap();
        assert_eq!((channel, kind, len), (7, DATA, 2));
        assert_eq!(&bytes[HEADER..], b"hi");
    }

    #[test]
    fn oversized_payload_is_rejected() {
        let too_big = vec![0u8; MAX_PAYLOAD + 1];
        assert_eq!(
            encode(1, DATA, &too_big),
            Err(CodecError::TooLarge(MAX_PAYLOAD + 1))
        );

        let mut header = [0u8; HEADER];
        header[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_header(&header).is_err());
    }

    #[test]
    fn resize_is_cols_then_rows_little_endian() {
        let bytes = resize(3, 80, 24).unwrap();
        assert_eq!(bytes[4], RESIZE);
        assert_eq!(&bytes[HEADER..], &[80, 0, 24, 0]);
    }

    #[test]
    fn credit_carries_consumed_bytes() {
        let bytes = credit(5, 4096).unwrap();
        assert_eq!(bytes[4], CREDIT);
        assert_eq!(
            u32::from_le_bytes(bytes[HEADER..].try_into().unwrap()),
            4096
        );
    }

    #[test]
    fn focus_uses_one_byte() {
        assert_eq!(focus(2, true).unwrap()[HEADER], 1);
        assert_eq!(focus(2, false).unwrap()[HEADER], 0);
    }

    #[test]
    fn close_has_empty_payload() {
        let bytes = close(4).unwrap();
        assert_eq!(bytes[4], CLOSE);
        assert_eq!(bytes.len(), HEADER);
    }

    #[test]
    fn facts_carries_display_json_verbatim() {
        let display = br#"{"fg":null,"bg":null,"cursor":null,"cell":[8,16]}"#;
        let bytes = facts(3, display).unwrap();
        assert_eq!(bytes[4], FACTS);
        assert_eq!(&bytes[HEADER..], display);
    }
}
