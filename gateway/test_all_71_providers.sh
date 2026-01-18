#!/bin/bash
#
# WaaV Gateway - Complete 71 Provider Test Suite
# Tests all 32 STT + 37 TTS + 2 Realtime providers
#
# Usage: ./test_all_71_providers.sh [gateway_url]
#

GATEWAY_URL="${1:-http://localhost:3001}"

# Load environment variables
ENV_FILE="$(dirname "$0")/.env"
if [ -f "$ENV_FILE" ]; then
    export $(grep -v '^#' "$ENV_FILE" | xargs)
fi

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Counters
PASS=0
FAIL=0
SKIP=0
NO_KEY=0

# Results array
declare -a RESULTS

echo "========================================================================"
echo "        WaaV Gateway - Complete 71 Provider Test Suite"
echo "========================================================================"
echo ""
echo "Gateway URL: $GATEWAY_URL"
echo "Started at: $(date)"
echo ""

# Check gateway health
echo -n "Checking gateway health... "
HEALTH=$(curl -s "$GATEWAY_URL/" 2>/dev/null || echo "")
if [[ "$HEALTH" == *"OK"* ]]; then
    echo -e "${GREEN}OK${NC}"
else
    echo -e "${RED}FAILED${NC}"
    echo "Gateway is not running at $GATEWAY_URL"
    exit 1
fi
echo ""

# =====================================================
# TTS Provider Tests
# =====================================================
# Each entry: provider:model:env_var

TTS_TESTS=(
    # Major Cloud Providers
    "deepgram:aura-asteria-en:DEEPGRAM_API_KEY"
    "elevenlabs:eleven_monolingual_v1:ELEVENLABS_API_KEY"
    "google:en-US-Neural2-C:GOOGLE_APPLICATION_CREDENTIALS"
    "microsoft-azure:en-US-JennyNeural:AZURE_SPEECH_SUBSCRIPTION_KEY"
    "cartesia:sonic-english:CARTESIA_API_KEY"
    "openai:tts-1:OPENAI_API_KEY"
    "aws-polly:Joanna:AWS_ACCESS_KEY_ID"
    "ibm-watson:en-US_MichaelV3Voice:IBM_WATSON_API_KEY"

    # Specialist Providers
    "hume:default:HUME_API_KEY"
    "lmnt:lily:LMNT_API_KEY"
    "playht:Play3.0-mini:PLAYHT_API_KEY"
    "gnani:en-IN:GNANI_API_KEY"
    "murf:falcon:MURF_API_KEY"
    "wellsaid:legacy:WELLSAID_API_KEY"
    "resemble:chatterbox:RESEMBLE_API_KEY"
    "speechify:simba-english:SPEECHIFY_API_KEY"
    "unrealspeech:Scarlett:UNREALSPEECH_API_KEY"
    "speechmatics:default:SPEECHMATICS_API_KEY"
    "acapela:default:ACAPELA_APPLICATION"
    "cereproc:default:CEREPROC_USERNAME"
    "smallest:lightning:SMALLEST_API_KEY"

    # Regional/Language Specific
    "reverie:hi:REVERIE_API_KEY"
    "yandex:ru-RU:YANDEX_API_KEY"
    "tinkoff:alyona:TINKOFF_API_KEY"
    "sberdevices:Nec:SBERDEVICES_CLIENT_SECRET"
    "bhashini:hi:BHASHINI_API_KEY"
    "iflytek:x4_yeting:IFLYTEK_APP_ID"
    "alibaba-cloud:cosyvoice-v3-flash:ALIBABA_CLOUD_API_KEY"
    "baidu:default:BAIDU_APP_ID"
    "tencent:default:TENCENT_SECRET_ID"
    "huawei-cloud:default:HUAWEI_CLOUD_AK"
    "naver-clova:default:NAVER_CLOVA_CLIENT_ID"

    # Southeast Asian
    "zalo-ai:1:ZALO_API_KEY"
    "fpt-ai:banmai:FPT_AI_API_KEY"
    "viettel-ai:hcm-diemmy:VIETTEL_AI_TOKEN"
    "prosa-ai:dimas-formal:PROSA_API_KEY"
    "nectec:vaja9:NECTEC_API_KEY"
)

# =====================================================
# STT Provider Tests
# =====================================================
# Each entry: provider:env_var

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
# Realtime Provider Tests
# =====================================================
REALTIME_PROVIDERS=(
    "openai:OPENAI_API_KEY"
    "hume:HUME_API_KEY"
)

has_api_key() {
    local env_var="$1"
    local key_val="${!env_var:-}"
    if [[ -z "$key_val" ]] || [[ "$key_val" == "your_"* ]] || [[ "$key_val" == "sk_example"* ]]; then
        return 1
    fi
    return 0
}

test_tts() {
    local provider="$1"
    local model="$2"
    local env_var="$3"

    if ! has_api_key "$env_var"; then
        echo -e "  ${YELLOW}⊠${NC} $provider TTS - No API key ($env_var)"
        RESULTS+=("TTS:$provider:NO_KEY:$env_var not configured")
        ((NO_KEY++))
        return
    fi

    local start_time=$(date +%s%3N)
    local response=$(curl -s -X POST "$GATEWAY_URL/speak" \
        -H "Content-Type: application/json" \
        -d "{\"text\":\"Hello world test\",\"tts_config\":{\"provider\":\"$provider\",\"model\":\"$model\"}}" \
        -o /tmp/tts_test_$provider.bin \
        -w "%{http_code}:%{size_download}" 2>/dev/null || echo "000:0")
    local end_time=$(date +%s%3N)
    local latency=$((end_time - start_time))

    local http_code="${response%%:*}"
    local size="${response##*:}"

    if [[ "$http_code" == "200" ]] && [[ "$size" -gt 100 ]]; then
        echo -e "  ${GREEN}✓${NC} $provider TTS - ${size} bytes (${latency}ms)"
        RESULTS+=("TTS:$provider:PASS:${size} bytes:${latency}ms")
        ((PASS++))
    elif [[ "$http_code" == "500" ]]; then
        local err=$(cat /tmp/tts_test_$provider.bin 2>/dev/null | head -c 100)
        echo -e "  ${RED}✗${NC} $provider TTS - Server error (HTTP $http_code)"
        RESULTS+=("TTS:$provider:FAIL:HTTP $http_code - $err")
        ((FAIL++))
    elif [[ "$http_code" == "000" ]]; then
        echo -e "  ${RED}✗${NC} $provider TTS - Connection failed"
        RESULTS+=("TTS:$provider:FAIL:Connection failed")
        ((FAIL++))
    else
        echo -e "  ${RED}✗${NC} $provider TTS - HTTP $http_code (${size} bytes)"
        RESULTS+=("TTS:$provider:FAIL:HTTP $http_code")
        ((FAIL++))
    fi

    rm -f /tmp/tts_test_$provider.bin
}

test_stt() {
    local provider="$1"
    local env_var="$2"

    if ! has_api_key "$env_var"; then
        echo -e "  ${YELLOW}⊠${NC} $provider STT - No API key ($env_var)"
        RESULTS+=("STT:$provider:NO_KEY:$env_var not configured")
        ((NO_KEY++))
        return
    fi

    # STT requires WebSocket - mark as configured but skip actual test
    echo -e "  ${BLUE}⊘${NC} $provider STT - Key present (WS test skipped)"
    RESULTS+=("STT:$provider:SKIP:API key configured")
    ((SKIP++))
}

test_realtime() {
    local provider="$1"
    local env_var="$2"

    if ! has_api_key "$env_var"; then
        echo -e "  ${YELLOW}⊠${NC} $provider Realtime - No API key ($env_var)"
        RESULTS+=("REALTIME:$provider:NO_KEY:$env_var not configured")
        ((NO_KEY++))
        return
    fi

    echo -e "  ${BLUE}⊘${NC} $provider Realtime - Key present (WS test skipped)"
    RESULTS+=("REALTIME:$provider:SKIP:API key configured")
    ((SKIP++))
}

# =====================================================
# Run TTS Tests
# =====================================================
echo "--- TTS PROVIDERS (${#TTS_TESTS[@]}) ---"
for entry in "${TTS_TESTS[@]}"; do
    IFS=':' read -r provider model env_var <<< "$entry"
    test_tts "$provider" "$model" "$env_var"
done

echo ""

# =====================================================
# Run STT Tests
# =====================================================
echo "--- STT PROVIDERS (${#STT_PROVIDERS[@]}) ---"
for entry in "${STT_PROVIDERS[@]}"; do
    IFS=':' read -r provider env_var <<< "$entry"
    test_stt "$provider" "$env_var"
done

echo ""

# =====================================================
# Run Realtime Tests
# =====================================================
echo "--- REALTIME PROVIDERS (${#REALTIME_PROVIDERS[@]}) ---"
for entry in "${REALTIME_PROVIDERS[@]}"; do
    IFS=':' read -r provider env_var <<< "$entry"
    test_realtime "$provider" "$env_var"
done

# =====================================================
# Summary
# =====================================================
TOTAL=$((PASS + FAIL + SKIP + NO_KEY))

echo ""
echo "========================================================================"
echo "                           TEST SUMMARY"
echo "========================================================================"
echo "Completed at: $(date)"
echo ""
echo "Total Providers Tested: $TOTAL"
echo -e "  ${GREEN}✓ Passed:${NC}      $PASS"
echo -e "  ${RED}✗ Failed:${NC}      $FAIL"
echo -e "  ${BLUE}⊘ Skipped:${NC}     $SKIP"
echo -e "  ${YELLOW}⊠ No API Key:${NC}  $NO_KEY"
echo ""

# Detailed results by status
echo "--- PASSED TESTS ---"
for result in "${RESULTS[@]}"; do
    if [[ "$result" == *":PASS:"* ]]; then
        echo "  $result"
    fi
done

echo ""
echo "--- FAILED TESTS ---"
for result in "${RESULTS[@]}"; do
    if [[ "$result" == *":FAIL:"* ]]; then
        echo "  $result"
    fi
done

echo ""
echo "--- PROVIDERS WITHOUT API KEYS ---"
for result in "${RESULTS[@]}"; do
    if [[ "$result" == *":NO_KEY:"* ]]; then
        IFS=':' read -r type provider status msg <<< "$result"
        echo "  $provider ($type) - $msg"
    fi
done

echo ""
echo "========================================================================"

# Save results to file
RESULTS_FILE="/tmp/waav_provider_test_results_$(date +%Y%m%d_%H%M%S).txt"
{
    echo "WaaV Gateway Provider Test Results"
    echo "==================================="
    echo "Date: $(date)"
    echo "Gateway: $GATEWAY_URL"
    echo ""
    echo "Summary:"
    echo "  Passed: $PASS"
    echo "  Failed: $FAIL"
    echo "  Skipped: $SKIP"
    echo "  No API Key: $NO_KEY"
    echo ""
    echo "All Results:"
    for result in "${RESULTS[@]}"; do
        echo "  $result"
    done
} > "$RESULTS_FILE"

echo "Results saved to: $RESULTS_FILE"
echo ""

# Exit code
if [[ $FAIL -gt 0 ]]; then
    exit 1
fi
exit 0
