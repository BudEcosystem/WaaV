//! Acapela Cloud API Message Types
//!
//! This module defines the request and response types for the Acapela Cloud API.

use serde::{Deserialize, Serialize};

// =============================================================================
// Authentication Messages
// =============================================================================

/// Login request body
#[derive(Debug, Clone, Serialize)]
pub struct LoginRequest {
    /// User email address
    pub email: String,
    /// User password
    pub password: String,
}

/// Login response
#[derive(Debug, Clone, Deserialize)]
pub struct LoginResponse {
    /// Session token for subsequent requests
    pub token: String,
}

/// Logout response
#[derive(Debug, Clone, Deserialize)]
pub struct LogoutResponse {
    /// Success message
    pub success: String,
}

// =============================================================================
// Error Messages
// =============================================================================

/// API error response
#[derive(Debug, Clone, Deserialize)]
pub struct AcapelaApiError {
    /// Error message
    #[serde(default)]
    pub error: Option<String>,
    /// Error detail
    #[serde(default)]
    pub detail: Option<String>,
    /// Error code
    #[serde(default)]
    pub code: Option<String>,
}

impl AcapelaApiError {
    /// Get the error message
    pub fn message(&self) -> String {
        self.error
            .clone()
            .or_else(|| self.detail.clone())
            .unwrap_or_else(|| "Unknown error".to_string())
    }
}

impl std::fmt::Display for AcapelaApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

// =============================================================================
// Event Types (for streaming responses)
// =============================================================================

/// Word position event for text highlighting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordEvent {
    /// The word text
    pub word: String,
    /// Start time in milliseconds
    pub start_time: u64,
    /// End time in milliseconds
    pub end_time: u64,
    /// Start sample position
    #[serde(default)]
    pub start_sample: Option<u64>,
    /// End sample position
    #[serde(default)]
    pub end_sample: Option<u64>,
}

/// Phoneme event with viseme data for lip-sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhonemeEvent {
    /// The phoneme text
    pub phoneme: String,
    /// Viseme code (Disney standard, 0-21)
    pub viseme: u8,
    /// Start time in milliseconds
    pub start_time: u64,
    /// End time in milliseconds
    pub end_time: u64,
}

/// Combined events response
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventsData {
    /// Word position events
    #[serde(rename = "Word", default)]
    pub words: Vec<WordEvent>,
    /// Phoneme/viseme events
    #[serde(rename = "Phoneme", default)]
    pub phonemes: Vec<PhonemeEvent>,
}

// =============================================================================
// Streaming Protocol
// =============================================================================

/// A chunk from the streaming response
#[derive(Debug, Clone)]
pub enum StreamChunk {
    /// Audio data chunk
    Audio(Vec<u8>),
    /// Events data chunk (JSON)
    Events(EventsData),
}

/// Parser for Acapela's streaming protocol
///
/// The streaming response uses a mixed protocol format:
/// ```text
/// type:size\n
/// content
/// ```
///
/// Where:
/// - `type` is "audio" or "events"
/// - `size` is the content length in bytes
/// - `content` is the raw audio bytes or JSON event data
#[derive(Debug, Default)]
pub struct StreamParser {
    /// Buffer for accumulating partial data
    buffer: Vec<u8>,
    /// Current chunk type being parsed
    current_type: Option<ChunkType>,
    /// Current chunk size being parsed
    current_size: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ChunkType {
    Audio,
    Events,
}

impl StreamParser {
    /// Create a new stream parser
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse incoming data and extract complete chunks
    ///
    /// Returns a vector of complete chunks parsed from the data.
    /// Partial chunks are buffered for the next call.
    pub fn parse(&mut self, data: &[u8]) -> Vec<StreamChunk> {
        let mut chunks = Vec::new();
        self.buffer.extend_from_slice(data);

        loop {
            // If we don't have a type/size yet, look for the header
            if self.current_type.is_none() {
                // Look for newline to find header end
                if let Some(newline_pos) = self.buffer.iter().position(|&b| b == b'\n') {
                    let header = String::from_utf8_lossy(&self.buffer[..newline_pos]);

                    // Parse "type:size" format
                    if let Some(colon_pos) = header.find(':') {
                        let chunk_type = &header[..colon_pos];
                        let size_str = &header[colon_pos + 1..];

                        let parsed_type = match chunk_type {
                            "audio" => Some(ChunkType::Audio),
                            "events" => Some(ChunkType::Events),
                            _ => None,
                        };

                        if let (Some(ct), Ok(size)) = (parsed_type, size_str.parse::<usize>()) {
                            self.current_type = Some(ct);
                            self.current_size = Some(size);
                            // Remove header from buffer
                            self.buffer = self.buffer[newline_pos + 1..].to_vec();
                        } else {
                            // Invalid header, skip it
                            self.buffer = self.buffer[newline_pos + 1..].to_vec();
                            continue;
                        }
                    } else {
                        // No colon found, might be partial header or plain audio
                        // For backward compatibility, treat as raw audio if no protocol detected
                        break;
                    }
                } else {
                    // No newline yet, need more data
                    break;
                }
            }

            // If we have type and size, check if we have enough data
            if let (Some(chunk_type), Some(size)) = (self.current_type, self.current_size) {
                if self.buffer.len() >= size {
                    let content = self.buffer[..size].to_vec();
                    self.buffer = self.buffer[size..].to_vec();

                    let chunk = match chunk_type {
                        ChunkType::Audio => StreamChunk::Audio(content),
                        ChunkType::Events => {
                            // Parse JSON events
                            match serde_json::from_slice::<EventsData>(&content) {
                                Ok(events) => StreamChunk::Events(events),
                                Err(_) => {
                                    // If JSON parsing fails, skip this chunk
                                    self.current_type = None;
                                    self.current_size = None;
                                    continue;
                                }
                            }
                        }
                    };

                    chunks.push(chunk);
                    self.current_type = None;
                    self.current_size = None;
                } else {
                    // Not enough data yet
                    break;
                }
            } else {
                break;
            }
        }

        chunks
    }

    /// Get remaining buffered data (for raw audio fallback)
    pub fn remaining(&mut self) -> Option<Vec<u8>> {
        if self.buffer.is_empty() {
            None
        } else {
            let data = std::mem::take(&mut self.buffer);
            Some(data)
        }
    }

    /// Check if there's partial data buffered or pending header parsed
    pub fn has_partial_data(&self) -> bool {
        !self.buffer.is_empty() || self.current_type.is_some()
    }

    /// Reset the parser state
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.current_type = None;
        self.current_size = None;
    }
}

// =============================================================================
// Viseme Mapping (Disney Standard)
// =============================================================================

/// Disney standard viseme codes for mouth shapes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Viseme {
    /// Silence - closed mouth
    Silence = 0,
    /// ae, ax, ah - open mouth
    Open = 1,
    /// aa - wide open
    WideOpen = 2,
    /// ao - rounded open
    RoundedOpen = 3,
    /// ey, eh, uh - half open
    HalfOpen = 4,
    /// er - R-colored vowel
    RColored = 5,
    /// y, iy, ih, ix - smile
    Smile = 6,
    /// w, uw - rounded
    Rounded = 7,
    /// ow - O shape
    OShape = 8,
    /// aw - wide O
    WideO = 9,
    /// oy - OI diphthong
    OIDiphthong = 10,
    /// ay - AI diphthong
    AIDiphthong = 11,
    /// h - breath
    Breath = 12,
    /// r - R consonant
    RConsonant = 13,
    /// l - L consonant
    LConsonant = 14,
    /// s, z - S/Z fricatives
    SZFricative = 15,
    /// sh, ch, jh, zh - SH affricate
    SHAffricate = 16,
    /// th, dh - TH dental
    THDental = 17,
    /// f, v - F/V labiodental
    FVLabiodental = 18,
    /// d, t, n - D/T/N alveolar
    DTNAlveolar = 19,
    /// k, g, ng - K/G/NG velar
    KGVelar = 20,
    /// p, b, m - P/B/M bilabial
    PBMBilabial = 21,
}

impl Viseme {
    /// Get viseme from code
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Silence),
            1 => Some(Self::Open),
            2 => Some(Self::WideOpen),
            3 => Some(Self::RoundedOpen),
            4 => Some(Self::HalfOpen),
            5 => Some(Self::RColored),
            6 => Some(Self::Smile),
            7 => Some(Self::Rounded),
            8 => Some(Self::OShape),
            9 => Some(Self::WideO),
            10 => Some(Self::OIDiphthong),
            11 => Some(Self::AIDiphthong),
            12 => Some(Self::Breath),
            13 => Some(Self::RConsonant),
            14 => Some(Self::LConsonant),
            15 => Some(Self::SZFricative),
            16 => Some(Self::SHAffricate),
            17 => Some(Self::THDental),
            18 => Some(Self::FVLabiodental),
            19 => Some(Self::DTNAlveolar),
            20 => Some(Self::KGVelar),
            21 => Some(Self::PBMBilabial),
            _ => None,
        }
    }

    /// Get description of the mouth shape
    pub fn description(&self) -> &'static str {
        match self {
            Self::Silence => "Closed mouth (silence)",
            Self::Open => "Open mouth (ae, ax, ah)",
            Self::WideOpen => "Wide open (aa)",
            Self::RoundedOpen => "Rounded open (ao)",
            Self::HalfOpen => "Half open (ey, eh, uh)",
            Self::RColored => "R-colored vowel (er)",
            Self::Smile => "Smile shape (y, iy, ih, ix)",
            Self::Rounded => "Rounded lips (w, uw)",
            Self::OShape => "O shape (ow)",
            Self::WideO => "Wide O (aw)",
            Self::OIDiphthong => "OI diphthong (oy)",
            Self::AIDiphthong => "AI diphthong (ay)",
            Self::Breath => "Breath (h)",
            Self::RConsonant => "R consonant",
            Self::LConsonant => "L consonant",
            Self::SZFricative => "S/Z fricative",
            Self::SHAffricate => "SH/CH affricate",
            Self::THDental => "TH dental",
            Self::FVLabiodental => "F/V labiodental",
            Self::DTNAlveolar => "D/T/N alveolar",
            Self::KGVelar => "K/G/NG velar",
            Self::PBMBilabial => "P/B/M bilabial",
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_request_serialization() {
        let request = LoginRequest {
            email: "user@example.com".to_string(),
            password: "password123".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"email\":\"user@example.com\""));
        assert!(json.contains("\"password\":\"password123\""));
    }

    #[test]
    fn test_login_response_deserialization() {
        let json = r#"{"token": "abc123xyz"}"#;
        let response: LoginResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.token, "abc123xyz");
    }

    #[test]
    fn test_error_response_deserialization() {
        let json = r#"{"error": "Invalid credentials", "code": "401"}"#;
        let error: AcapelaApiError = serde_json::from_str(json).unwrap();
        assert_eq!(error.error, Some("Invalid credentials".to_string()));
        assert_eq!(error.code, Some("401".to_string()));
        assert_eq!(error.message(), "Invalid credentials");
    }

    #[test]
    fn test_error_response_with_detail() {
        let json = r#"{"detail": "Account inactive"}"#;
        let error: AcapelaApiError = serde_json::from_str(json).unwrap();
        assert_eq!(error.message(), "Account inactive");
    }

    #[test]
    fn test_word_event_deserialization() {
        let json = r#"{
            "word": "Hello",
            "start_time": 0,
            "end_time": 500,
            "start_sample": 0,
            "end_sample": 8000
        }"#;

        let event: WordEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.word, "Hello");
        assert_eq!(event.start_time, 0);
        assert_eq!(event.end_time, 500);
        assert_eq!(event.start_sample, Some(0));
        assert_eq!(event.end_sample, Some(8000));
    }

    #[test]
    fn test_phoneme_event_deserialization() {
        let json = r#"{
            "phoneme": "h",
            "viseme": 12,
            "start_time": 0,
            "end_time": 100
        }"#;

        let event: PhonemeEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.phoneme, "h");
        assert_eq!(event.viseme, 12);
        assert_eq!(event.start_time, 0);
        assert_eq!(event.end_time, 100);
    }

    #[test]
    fn test_events_data_deserialization() {
        let json = r#"{
            "Word": [
                {"word": "Hello", "start_time": 0, "end_time": 500}
            ],
            "Phoneme": [
                {"phoneme": "h", "viseme": 12, "start_time": 0, "end_time": 100}
            ]
        }"#;

        let events: EventsData = serde_json::from_str(json).unwrap();
        assert_eq!(events.words.len(), 1);
        assert_eq!(events.phonemes.len(), 1);
        assert_eq!(events.words[0].word, "Hello");
        assert_eq!(events.phonemes[0].phoneme, "h");
    }

    #[test]
    fn test_stream_parser_audio_chunk() {
        let mut parser = StreamParser::new();

        // Simulate audio chunk: "audio:5\nhello"
        let data = b"audio:5\nhello";
        let chunks = parser.parse(data);

        assert_eq!(chunks.len(), 1);
        if let StreamChunk::Audio(audio) = &chunks[0] {
            assert_eq!(audio, b"hello");
        } else {
            panic!("Expected Audio chunk");
        }
    }

    #[test]
    fn test_stream_parser_events_chunk() {
        let mut parser = StreamParser::new();

        let events_json = r#"{"Word":[],"Phoneme":[]}"#;
        let header = format!("events:{}\n", events_json.len());
        let data = format!("{}{}", header, events_json);

        let chunks = parser.parse(data.as_bytes());

        assert_eq!(chunks.len(), 1);
        if let StreamChunk::Events(events) = &chunks[0] {
            assert!(events.words.is_empty());
            assert!(events.phonemes.is_empty());
        } else {
            panic!("Expected Events chunk");
        }
    }

    #[test]
    fn test_stream_parser_multiple_chunks() {
        let mut parser = StreamParser::new();

        // Two audio chunks
        let data = b"audio:3\nabcaudio:3\ndef";
        let chunks = parser.parse(data);

        assert_eq!(chunks.len(), 2);
        if let StreamChunk::Audio(audio) = &chunks[0] {
            assert_eq!(audio, b"abc");
        }
        if let StreamChunk::Audio(audio) = &chunks[1] {
            assert_eq!(audio, b"def");
        }
    }

    #[test]
    fn test_stream_parser_partial_data() {
        let mut parser = StreamParser::new();

        // First part: header only
        let chunks = parser.parse(b"audio:10\n");
        assert!(chunks.is_empty());
        assert!(parser.has_partial_data());

        // Second part: partial content
        let chunks = parser.parse(b"hello");
        assert!(chunks.is_empty());

        // Third part: remaining content
        let chunks = parser.parse(b"world");
        assert_eq!(chunks.len(), 1);
        if let StreamChunk::Audio(audio) = &chunks[0] {
            assert_eq!(audio, b"helloworld");
        }
    }

    #[test]
    fn test_stream_parser_reset() {
        let mut parser = StreamParser::new();
        parser.parse(b"audio:100\npartial");
        assert!(parser.has_partial_data());

        parser.reset();
        assert!(!parser.has_partial_data());
    }

    #[test]
    fn test_viseme_from_code() {
        assert_eq!(Viseme::from_code(0), Some(Viseme::Silence));
        assert_eq!(Viseme::from_code(6), Some(Viseme::Smile));
        assert_eq!(Viseme::from_code(21), Some(Viseme::PBMBilabial));
        assert_eq!(Viseme::from_code(22), None);
    }

    #[test]
    fn test_viseme_description() {
        assert_eq!(Viseme::Silence.description(), "Closed mouth (silence)");
        assert_eq!(Viseme::Smile.description(), "Smile shape (y, iy, ih, ix)");
        assert_eq!(Viseme::PBMBilabial.description(), "P/B/M bilabial");
    }

    #[test]
    fn test_api_error_display() {
        let error = AcapelaApiError {
            error: Some("Test error".to_string()),
            detail: None,
            code: None,
        };
        assert_eq!(format!("{}", error), "Test error");
    }

    #[test]
    fn test_api_error_fallback() {
        let error = AcapelaApiError {
            error: None,
            detail: None,
            code: None,
        };
        assert_eq!(error.message(), "Unknown error");
    }
}
