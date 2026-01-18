#!/bin/bash
#
# WaaV Gateway Provider Test Script
# Tests all 71 providers (32 STT + 37 TTS + 2 Realtime)
#
# Usage: ./test_providers.sh [gateway_url]
# Default: http://localhost:3001

set -e

GATEWAY_URL="${1:-http://localhost:3001}"
PASS=0
FAIL=0
SKIP=0
NO_KEY=0

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Test audio file (generate a simple WAV file)
TEST_AUDIO="/tmp/test_audio.wav"

generate_test_audio() {
    # Generate a simple 1-second 16kHz mono WAV file with silence
    python3 << 'EOF'
import struct
import wave

# Create 1 second of silence at 16kHz
sample_rate = 16000
duration = 1.0
num_samples = int(sample_rate * duration)

with wave.open('/tmp/test_audio.wav', 'w') as wav:
    wav.setnchannels(1)
    wav.setsampwidth(2)
    wav.setframerate(sample_rate)
    # Write silence
    for _ in range(num_samples):
        wav.writeframes(struct.pack('<h', 0))
EOF
}

echo "========================================"
echo "  WaaV Gateway Provider Test Suite"
echo "========================================"
echo ""
echo "Gateway URL: $GATEWAY_URL"
echo ""

# Check gateway health
echo -n "Checking gateway health... "
HEALTH=$(curl -s "$GATEWAY_URL/" 2>/dev/null || echo "")
if [[ "$HEALTH" == *"OK"* ]]; then
    echo -e "${GREEN}OK${NC}"
else
    echo -e "${RED}FAILED${NC}"
    echo "Gateway is not running at $GATEWAY_URL"
    echo "Start it with: cargo run --release"
    exit 1
fi

# Generate test audio
echo -n "Generating test audio... "
generate_test_audio
echo "Done"
echo ""

# =====================================================
# STT Providers (32)
# =====================================================
STT_PROVIDERS=(
    "deepgram:DEEPGRAM_API_KEY"
    "google:GOOGLE_APPLICATION_CREDENTIALS"
    "elevenlabs:ELEVENLABS_API_KEY"
    "microsoft-azure:AZURE_SPEECH_SUBSCRIPTION_KEY"
    "cartesia:CARTESIA_API_KEY"
    "openai:OPENAI_API_KEY"
    "assemblyai:ASSEMBLYAI_API_KEY"
    "aws-transcribe:AWS_ACCESS_KEY_ID"
    "ibm-watson:IBM_WATSON_API_KEY"
    "groq:GROQ_API_KEY"
    "gnani:GNANI_API_KEY"
    "sarvam:SARVAM_API_KEY"
    "speechmatics:SPEECHMATICS_API_KEY"
    "gladia:GLADIA_API_KEY"
    "revai:REVAI_API_KEY"
    "phonexia:PHONEXIA_API_KEY"
    "reverie:REVERIE_API_KEY"
    "yandex:YANDEX_API_KEY"
    "tinkoff:TINKOFF_API_KEY"
    "sberdevices:SBERDEVICES_CLIENT_SECRET"
    "bhashini:BHASHINI_API_KEY"
    "iflytek:IFLYTEK_APP_ID"
    "alibaba-cloud:ALIBABA_CLOUD_API_KEY"
    "baidu:BAIDU_APP_ID"
    "tencent:TENCENT_SECRET_ID"
    "huawei-cloud:HUAWEI_CLOUD_AK"
    "naver-clova:NAVER_CLOVA_CLIENT_ID"
    "amivoice:AMIVOICE_APP_KEY"
    "fpt-ai:FPT_AI_API_KEY"
    "viettel-ai:VIETTEL_AI_TOKEN"
    "prosa-ai:PROSA_API_KEY"
    "nectec:NECTEC_API_KEY"
)

# =====================================================
# TTS Providers (37)
# =====================================================
TTS_PROVIDERS=(
    "deepgram:DEEPGRAM_API_KEY"
    "elevenlabs:ELEVENLABS_API_KEY"
    "google:GOOGLE_APPLICATION_CREDENTIALS"
    "microsoft-azure:AZURE_SPEECH_SUBSCRIPTION_KEY"
    "cartesia:CARTESIA_API_KEY"
    "openai:OPENAI_API_KEY"
    "aws-polly:AWS_ACCESS_KEY_ID"
    "ibm-watson:IBM_WATSON_API_KEY"
    "hume:HUME_API_KEY"
    "lmnt:LMNT_API_KEY"
    "playht:PLAYHT_API_KEY"
    "gnani:GNANI_API_KEY"
    "murf:MURF_API_KEY"
    "wellsaid:WELLSAID_API_KEY"
    "resemble:RESEMBLE_API_KEY"
    "speechify:SPEECHIFY_API_KEY"
    "unrealspeech:UNREALSPEECH_API_KEY"
    "speechmatics:SPEECHMATICS_API_KEY"
    "acapela:ACAPELA_APPLICATION"
    "cereproc:CEREPROC_USERNAME"
    "reverie:REVERIE_API_KEY"
    "yandex:YANDEX_API_KEY"
    "smallest:SMALLEST_API_KEY"
    "tinkoff:TINKOFF_API_KEY"
    "sberdevices:SBERDEVICES_CLIENT_SECRET"
    "bhashini:BHASHINI_API_KEY"
    "iflytek:IFLYTEK_APP_ID"
    "alibaba-cloud:ALIBABA_CLOUD_API_KEY"
    "baidu:BAIDU_APP_ID"
    "tencent:TENCENT_SECRET_ID"
    "huawei-cloud:HUAWEI_CLOUD_AK"
    "naver-clova:NAVER_CLOVA_CLIENT_ID"
    "zalo-ai:ZALO_API_KEY"
    "fpt-ai:FPT_AI_API_KEY"
    "viettel-ai:VIETTEL_AI_TOKEN"
    "prosa-ai:PROSA_API_KEY"
    "nectec:NECTEC_API_KEY"
)

# =====================================================
# Realtime Providers (2)
# =====================================================
REALTIME_PROVIDERS=(
    "openai:OPENAI_API_KEY"
    "hume:HUME_API_KEY"
)

test_tts_provider() {
    local provider="$1"
    local env_var="$2"

    # Check if API key is configured
    local key_val="${!env_var:-}"
    if [[ -z "$key_val" ]] || [[ "$key_val" == "your_"* ]] || [[ "$key_val" == "sk_example"* ]]; then
        echo -e "  ${YELLOW}⊠${NC} $provider (TTS) - No API key ($env_var)"
        ((NO_KEY++))
        return
    fi

    # Test via gateway /speak endpoint
    local response=$(curl -s -w "\n%{http_code}" -X POST "$GATEWAY_URL/speak" \
        -H "Content-Type: application/json" \
        -d "{\"text\":\"Hello\",\"provider\":\"$provider\"}" 2>/dev/null || echo "000")

    local http_code=$(echo "$response" | tail -n1)
    local body=$(echo "$response" | sed '$d')

    if [[ "$http_code" == "200" ]]; then
        local size=${#body}
        if [[ $size -gt 100 ]]; then
            echo -e "  ${GREEN}✓${NC} $provider (TTS) - ${size} bytes"
            ((PASS++))
        else
            echo -e "  ${RED}✗${NC} $provider (TTS) - Too small response (${size} bytes)"
            ((FAIL++))
        fi
    elif [[ "$http_code" == "000" ]]; then
        echo -e "  ${RED}✗${NC} $provider (TTS) - Connection failed"
        ((FAIL++))
    else
        echo -e "  ${RED}✗${NC} $provider (TTS) - HTTP $http_code"
        ((FAIL++))
    fi
}

test_stt_provider() {
    local provider="$1"
    local env_var="$2"

    # Check if API key is configured
    local key_val="${!env_var:-}"
    if [[ -z "$key_val" ]] || [[ "$key_val" == "your_"* ]]; then
        echo -e "  ${YELLOW}⊠${NC} $provider (STT) - No API key ($env_var)"
        ((NO_KEY++))
        return
    fi

    # STT requires WebSocket connection - mark as having key but skip test
    echo -e "  ${BLUE}⊘${NC} $provider (STT) - API key present (WebSocket test skipped)"
    ((SKIP++))
}

test_realtime_provider() {
    local provider="$1"
    local env_var="$2"

    # Check if API key is configured
    local key_val="${!env_var:-}"
    if [[ -z "$key_val" ]] || [[ "$key_val" == "your_"* ]]; then
        echo -e "  ${YELLOW}⊠${NC} $provider (Realtime) - No API key ($env_var)"
        ((NO_KEY++))
        return
    fi

    echo -e "  ${BLUE}⊘${NC} $provider (Realtime) - API key present (WebSocket test skipped)"
    ((SKIP++))
}

# =====================================================
# Run Tests
# =====================================================

echo "--- STT PROVIDERS (${#STT_PROVIDERS[@]}) ---"
for entry in "${STT_PROVIDERS[@]}"; do
    IFS=':' read -r provider env_var <<< "$entry"
    test_stt_provider "$provider" "$env_var"
done

echo ""
echo "--- TTS PROVIDERS (${#TTS_PROVIDERS[@]}) ---"
for entry in "${TTS_PROVIDERS[@]}"; do
    IFS=':' read -r provider env_var <<< "$entry"
    test_tts_provider "$provider" "$env_var"
done

echo ""
echo "--- REALTIME PROVIDERS (${#REALTIME_PROVIDERS[@]}) ---"
for entry in "${REALTIME_PROVIDERS[@]}"; do
    IFS=':' read -r provider env_var <<< "$entry"
    test_realtime_provider "$provider" "$env_var"
done

# =====================================================
# Summary
# =====================================================
echo ""
echo "========================================"
echo "               SUMMARY"
echo "========================================"
TOTAL=$((PASS + FAIL + SKIP + NO_KEY))
echo "Total Providers: $TOTAL"
echo -e "  ${GREEN}✓${NC} Passed:     $PASS"
echo -e "  ${RED}✗${NC} Failed:     $FAIL"
echo -e "  ${BLUE}⊘${NC} Skipped:    $SKIP"
echo -e "  ${YELLOW}⊠${NC} No API Key: $NO_KEY"
echo "========================================"

# Cleanup
rm -f "$TEST_AUDIO"

# Exit code based on failures
if [[ $FAIL -gt 0 ]]; then
    exit 1
fi
exit 0
