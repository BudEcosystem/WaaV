/*
 * WaaV patch: see livekit/network_policy.h.
 */

#include "livekit/network_policy.h"

#include <cstdlib>
#include <mutex>
#include <string>
#include <vector>

#include "rtc_base/ip_address.h"
#include "rtc_base/network_constants.h"

namespace livekit {
namespace {

struct Prefix {
  webrtc::IPAddress ip;
  int length;
};

std::mutex g_policy_mutex;
std::vector<Prefix> g_prefixes;
bool g_include_loopback = false;

bool parse_prefix(const std::string& text, Prefix* out) {
  const auto slash = text.find('/');
  webrtc::IPAddress ip;
  if (!webrtc::IPFromString(text.substr(0, slash), &ip)) {
    return false;
  }
  const int max = ip.family() == AF_INET ? 32 : 128;
  int length = max;
  if (slash != std::string::npos) {
    const std::string bits = text.substr(slash + 1);
    char* end = nullptr;
    const long parsed = std::strtol(bits.c_str(), &end, 10);
    if (bits.empty() || *end != '\0' || parsed < 0 || parsed > max) {
      return false;
    }
    length = static_cast<int>(parsed);
  }
  *out = Prefix{webrtc::TruncateIP(ip, length), length};
  return true;
}

bool in_prefix(const webrtc::IPAddress& ip, const Prefix& p) {
  return ip.family() == p.ip.family() && webrtc::TruncateIP(ip, p.length) == p.ip;
}

// Gathers only on networks whose best address is in an allowed prefix.
class PolicyNetworkManager : public webrtc::BasicNetworkManager {
 public:
  using webrtc::BasicNetworkManager::BasicNetworkManager;

  std::vector<const webrtc::Network*> GetNetworks() const override {
    std::vector<const webrtc::Network*> all =
        webrtc::BasicNetworkManager::GetNetworks();
    std::vector<Prefix> prefixes;
    {
      std::lock_guard<std::mutex> lock(g_policy_mutex);
      prefixes = g_prefixes;
    }
    if (prefixes.empty()) {
      return all;
    }
    std::vector<const webrtc::Network*> kept;
    for (const webrtc::Network* network : all) {
      const webrtc::IPAddress ip = network->GetBestIP();
      for (const Prefix& p : prefixes) {
        if (in_prefix(ip, p)) {
          kept.push_back(network);
          break;
        }
      }
    }
    return kept;
  }

  // The "any address" networks bind 0.0.0.0 / [::], i.e. every interface: never under a policy.
  std::vector<const webrtc::Network*> GetAnyAddressNetworks() override {
    {
      std::lock_guard<std::mutex> lock(g_policy_mutex);
      if (!g_prefixes.empty()) {
        return {};
      }
    }
    return webrtc::BasicNetworkManager::GetAnyAddressNetworks();
  }
};

}  // namespace

void set_ice_network_policy(rust::Vec<rust::String> cidrs, bool include_loopback) {
  std::vector<Prefix> parsed;
  for (const rust::String& cidr : cidrs) {
    Prefix p;
    if (parse_prefix(std::string(cidr), &p)) {
      parsed.push_back(p);
    }
  }
  std::lock_guard<std::mutex> lock(g_policy_mutex);
  g_prefixes = std::move(parsed);
  g_include_loopback = include_loopback;
}

int ice_network_ignore_mask() {
  std::lock_guard<std::mutex> lock(g_policy_mutex);
  return g_include_loopback ? 0 : webrtc::kDefaultNetworkIgnoreMask;
}

std::unique_ptr<webrtc::NetworkManager> create_policy_network_manager(
    const webrtc::Environment& env,
    webrtc::SocketFactory* socket_factory) {
  return std::make_unique<PolicyNetworkManager>(env, socket_factory);
}

}  // namespace livekit
