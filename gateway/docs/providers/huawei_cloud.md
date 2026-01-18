# Huawei Cloud Speech Interaction Service (SIS) Integration

## Overview

Huawei Cloud Speech Interaction Service (SIS) is a comprehensive cloud-based speech processing platform that provides:
- **Automatic Speech Recognition (ASR)**: Short Sentence Recognition, Audio File Transcription, Real-Time ASR (RASR)
- **Text-to-Speech (TTS)**: Standard TTS, Real-Time TTS, TTS Customization (TTSC)

## Product Features

### ASR Services

| Service | Description | Duration Limit | Use Case |
|---------|-------------|----------------|----------|
| Short Sentence Recognition | HTTP-based batch recognition | Max 30 seconds | Voice commands, short queries |
| Audio File Transcription | Batch processing of audio files | Hours-long files | Meeting transcription, podcasts |
| Real-Time ASR (RASR) | WebSocket streaming recognition | Streaming: 1 min, Continuous: 5 hours | Live captioning, call centers |

### TTS Services

| Service | Description | Features |
|---------|-------------|----------|
| Standard TTS | HTTP-based synthesis | 11+ standard voices, 30+ premium voices |
| Real-Time TTS | WebSocket streaming | Low-latency streaming synthesis |
| TTS Customization (TTSC) | Custom voice cloning | Brand-specific voice creation |

## Supported Languages

### ASR Languages
- **Chinese Mandarin** (primary)
- **Chinese Dialects**: Cantonese, Sichuanese, Hokkienese (Minnan)
- **Minority Languages**: Mongolian, Tibetan, Uyghur

### TTS Languages
- **Chinese** (primary)
- **English**

## Voices

### Standard Voices (普通发音人)
| Voice ID | Name | Gender | Style |
|----------|------|--------|-------|
| chinese_xiaoqi_common | 小琪 | Female | Standard |
| chinese_xiaowen_common | 小雯 | Female | Soft |
| chinese_xiaoyan_common | 小燕 | Female | Gentle |
| chinese_xiaoqian_common | 小倩 | Female | Mature |
| chinese_xiaojing_common | 小婧 | Female | Lively |
| chinese_xiaoyu_common | 小宇 | Male | Standard |
| chinese_xiaosong_common | 小宋 | Male | Passionate |
| chinese_xiaowang_common | 小王 | Child | Standard |
| chinese_xiaodai_common | 小呆 | Child | Cute |
| chinese_cameal_common | Cameal | Female | English |

### Premium Voices (精品发音人)
Available only in cn-north-4 and cn-east-3 regions. 30+ voices including:
- 华小夏, 华小唯, 华晓刚 (specialized styles: sales, customer service, news, storytelling)
- Note: Premium voices do not support pitch adjustment

## Audio Formats

### ASR Audio Formats
| Format | Description |
|--------|-------------|
| pcm8k16bit | PCM, 8kHz sampling, 16-bit |
| pcm16k16bit | PCM, 16kHz sampling, 16-bit |
| wav | WAV format |
| amr | AMR format |
| amr-wb | AMR-WB format |
| mp3 | MP3 format |
| aac | AAC format |
| ogg-opus | OGG Opus format |
| m4a | M4A format |

### TTS Audio Formats
| Format | Description |
|--------|-------------|
| wav | WAV (default) |
| mp3 | MP3 |
| pcm | Raw PCM |

### Sample Rates
- **8000 Hz** (default for telephony)
- **16000 Hz** (default for general use)

## API Endpoints

### Regional Endpoints

| Region | Region Code | STT/TTS Endpoint |
|--------|-------------|------------------|
| CN North-Beijing4 | cn-north-4 | sis.cn-north-4.myhuaweicloud.com |
| CN East-Shanghai2 | cn-east-3 | sis.cn-east-3.myhuaweicloud.com |
| AP-Singapore | ap-southeast-3 | sis-ext.ap-southeast-3.myhuaweicloud.com |
| CN-Hong Kong | ap-southeast-1 | sis-ext.ap-southeast-1.myhuaweicloud.com |
| AP-Bangkok | ap-southeast-2 | sis-ext.ap-southeast-2.myhuaweicloud.com |

### API URLs

#### Short Sentence Recognition (HTTP)
```
POST https://{endpoint}/v1/{project_id}/asr/short-audio
```

#### Real-Time ASR (WebSocket)
```
wss://{endpoint}/v1/{project_id}/rasr/short-stream    # Streaming mode (< 1 min)
wss://{endpoint}/v1/{project_id}/rasr/continue-stream # Continuous mode (< 5 hours)
```

#### Standard TTS (HTTP)
```
POST https://{endpoint}/v1/{project_id}/tts
```

#### Real-Time TTS (WebSocket)
```
wss://{endpoint}/v1/{project_id}/rtts
```

## Authentication

### Token-Based Authentication
Huawei Cloud uses IAM token-based authentication:

1. **Obtain Token**: Call IAM authentication endpoint
2. **Use Token**: Include `X-Auth-Token` header in all requests

```
POST https://iam.{region}.myhuaweicloud.com/v3/auth/tokens
```

### AK/SK Authentication (SDK)
For SDK usage, use Access Key (AK) and Secret Key (SK):
- Store AK/SK securely in environment variables or encrypted config
- SDK handles signature generation automatically

## Request/Response Formats

### Short Sentence Recognition Request
```json
{
  "data": "<base64-encoded-audio>",
  "config": {
    "audio_format": "pcm16k16bit",
    "property": "chinese_16k_general",
    "add_punc": "yes",
    "digit_norm": "yes",
    "vocabulary_id": "<optional-hotword-table-id>",
    "need_word_info": "no"
  }
}
```

### Short Sentence Recognition Response
```json
{
  "trace_id": "<trace-id>",
  "result": {
    "text": "识别结果文本",
    "score": 0.95
  }
}
```

### Standard TTS Request
```json
{
  "text": "要合成的文本内容（最多500字符）",
  "config": {
    "audio_format": "wav",
    "sample_rate": "16000",
    "property": "chinese_xiaoyan_common",
    "speed": 0,
    "pitch": 0,
    "volume": 50
  }
}
```

### Standard TTS Response
```json
{
  "trace_id": "<trace-id>",
  "result": {
    "data": "<base64-encoded-audio>"
  }
}
```

### Real-Time TTS WebSocket Commands
```json
// Start synthesis
{"command": "START", "text": "要合成的文本", "config": {"audio_format": "pcm", "property": "chinese_xiaoyu_common", "sample_rate": "8000"}}

// Server responses: binary audio frames
// End signal from server indicates completion
```

### Real-Time ASR WebSocket Commands
```json
// Start recognition
{"command": "START", "config": {"audio_format": "pcm16k16bit", "property": "chinese_16k_general", "add_punc": "yes"}}

// Send audio (binary frames)

// End recognition
{"command": "END"}

// Server responses
{"resp_type": "RESULT", "trace_id": "...", "result": {"text": "...", "is_final": true/false}}
```

## Configuration Parameters

### ASR Property Format
Pattern: `{language}_{sample_rate}_{domain}`

| Property | Description |
|----------|-------------|
| chinese_8k_general | Mandarin, 8kHz, General |
| chinese_16k_general | Mandarin, 16kHz, General |
| chinese_8k_common | Mandarin, 8kHz, Common |
| chinese_16k_common | Mandarin, 16kHz, Common |

### TTS Configuration

| Parameter | Range | Default | Description |
|-----------|-------|---------|-------------|
| speed | -500 to 500 | 0 | Speech rate |
| pitch | -500 to 500 | 0 | Voice pitch (not available for premium voices) |
| volume | 0 to 100 | 50 | Volume level |

## Pricing (China Region)

| Service | Pricing |
|---------|---------|
| Short Sentence Recognition | ¥2.50 per 1,000 calls |
| Real-Time ASR (RASR) | ¥1.20 per hour |
| ASR Customization | ¥0.004 per call |
| Standard TTS | ¥4.00 per 1,000 calls |

Note: International region pricing may vary. Contact Huawei Cloud sales for details.

## Constraints and Limitations

### ASR Limitations
- Short sentence recognition: Max 30 seconds audio
- Streaming mode: Max 1 minute per session
- Continuous mode: Max 5 hours per session
- WebSocket connection timeout: 5 hours maximum

### TTS Limitations
- Text length: Max 500 characters per request
- One text per WebSocket connection
- Premium voices only in cn-north-4 and cn-east-3 regions

## Error Codes

| Code | Description |
|------|-------------|
| SIS.0001 | Authentication failed |
| SIS.0002 | Token expired |
| SIS.0003 | Invalid parameter |
| SIS.0004 | Audio format mismatch |
| SIS.0005 | Service unavailable |
| SIS.0006 | Rate limit exceeded |
| SIS.0007 | Quota exceeded |

## Implementation Plan

### Architecture Decision
Use **Core Integration** approach (not dynamic plugin) because:
1. Huawei Cloud requires IAM token refresh mechanism
2. WebSocket protocol for real-time STT/TTS
3. Complex authentication flow

### STT Implementation
1. Create `src/core/stt/huawei_cloud/` module
2. Implement `HuaweiCloudStt` with:
   - Token-based authentication with auto-refresh
   - WebSocket client for RASR
   - HTTP client for short sentence recognition
   - Support for all audio formats

### TTS Implementation
1. Create `src/core/tts/huawei_cloud/` module
2. Implement `HuaweiCloudTts` with:
   - WebSocket client for Real-Time TTS
   - HTTP client for Standard TTS
   - All voice properties support

### Files to Create
```
src/core/stt/huawei_cloud/
├── mod.rs          # Module exports, constants
├── config.rs       # HuaweiCloudSttConfig, regions, audio formats
├── auth.rs         # IAM token management
├── messages.rs     # WebSocket message types
└── client.rs       # HuaweiCloudStt implementation

src/core/tts/huawei_cloud/
├── mod.rs          # Module exports, constants
├── config.rs       # HuaweiCloudTtsConfig, voices, audio formats
└── provider.rs     # HuaweiCloudTts implementation
```

## References

- [Huawei Cloud SIS Overview](https://support.huaweicloud.com/intl/en-us/productdesc-sis/sis_01_0001.html)
- [SIS API Reference](https://support.huaweicloud.com/intl/en-us/api-sis/)
- [SIS SDK Reference](https://support.huaweicloud.com/intl/en-us/sdkreference-sis/)
- [Regions and Endpoints](https://developer.huaweicloud.com/intl/en-us/endpoint)
- [IAM Authentication](https://support.huaweicloud.com/intl/en-us/api-iam/iam_02_0510.html)
