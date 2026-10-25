/*
 * WaaV patch: which local networks WebRTC gathers ICE candidates on.
 *
 * libwebrtc enumerates every interface (LAN, Docker bridges, IPv6 ULA, VPNs) and skips loopback.
 * A deployment whose LiveKit server is on loopback, or that wants media on chosen addresses only,
 * sets a policy here before its first peer connection: the allowed prefixes (empty = every
 * network, as before) and whether loopback is gathered. The policy is read each time networks
 * are enumerated, so it applies to every peer connection created after it is set.
 */

#pragma once

#include <memory>

#include "api/environment/environment.h"
#include "rtc_base/network.h"
#include "rust/cxx.h"

namespace livekit {

// `cidrs`: "ip/len" prefixes (IPv4 or IPv6); a malformed entry is skipped. `include_loopback`:
// gather loopback interfaces (libwebrtc ignores them by default).
void set_ice_network_policy(rust::Vec<rust::String> cidrs, bool include_loopback);

// The adapter types libwebrtc should ignore under the current policy.
int ice_network_ignore_mask();

// A BasicNetworkManager whose networks are filtered by the policy.
std::unique_ptr<webrtc::NetworkManager> create_policy_network_manager(
    const webrtc::Environment& env,
    webrtc::SocketFactory* socket_factory);

}  // namespace livekit
