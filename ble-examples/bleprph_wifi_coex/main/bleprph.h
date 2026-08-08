// Copyright 2015-2020 The Apache Software Foundation
// Modifications Copyright 2017-2020 Espressif Systems (Shanghai) CO., LTD.
//
// Portions of this software were developed at Runtime Inc, copyright 2015.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#ifndef H_BLEPRPH_
#define H_BLEPRPH_

#include <stdbool.h>
#include "nimble/ble.h"
#include "modlog/modlog.h"
#ifdef __cplusplus
extern "C" {
#endif

struct ble_hs_cfg;
struct ble_gatt_register_ctxt;

/** GATT server. */
#define GATT_SVR_SVC_ALERT_UUID               0x1811

/* Wi-Fi provisioning limits, from the 802.11 spec: SSID is at most 32 bytes,
 * a WPA passphrase at most 63. Buffers add one byte for a NUL terminator. */
#define MONOCLE_SSID_MAX_LEN                  32
#define MONOCLE_PASS_MAX_LEN                  63

/* Values reported over the wifi_state characteristic. Keep in sync with
 * WifiState in src-tauri/src/ble.rs and docs/protocol.md. */
enum monocle_wifi_state {
    MONOCLE_WIFI_IDLE       = 0,  /* no credentials yet */
    MONOCLE_WIFI_CONNECTING = 1,
    MONOCLE_WIFI_CONNECTED  = 2,  /* followed by 4 IPv4 bytes, big-endian */
    MONOCLE_WIFI_FAILED     = 3,  /* followed by 1 reason byte */
};

void gatt_svr_register_cb(struct ble_gatt_register_ctxt *ctxt, void *arg);
int gatt_svr_init(void);

/* Connection bookkeeping, called from the GAP event handler in main.c. The
 * GATT layer needs the current connection to send notifications on. */
void gatt_svr_on_connect(uint16_t conn_handle);
void gatt_svr_on_disconnect(void);
void gatt_svr_on_subscribe(uint16_t conn_handle, uint16_t attr_handle,
                           int cur_notify);

/* Push a wifi_state notification. No-op when nobody is subscribed. */
void gatt_svr_notify_wifi_state(uint8_t state, const void *extra,
                                uint8_t extra_len);

/* Implemented in main.c. Joins a network, powering the radio up first if the
 * idle timer had taken it down. Returns 0 if the attempt started; the outcome
 * arrives as wifi_state notifications. Blocks — never call it from the BLE
 * host task; the GATT write handler uses wifi_prov_request_join() instead. */
int wifi_prov_connect(const char *ssid, const char *pass);

/* Queues a join on a worker task. Called by the GATT write handler once
 * credentials have been received and validated; the ATT response means the
 * request was accepted, not that the network was joined. */
void wifi_prov_request_join(const char *ssid, const char *pass);

/* Re-join using credentials already stored in NVS, powering the radio back
 * up if it was shut down. Returns 0 if the attempt started. */
int wifi_prov_resume(void);

/* Close the data plane and power the Wi-Fi radio down. This is what keeps
 * Wi-Fi's duty cycle low; BLE is unaffected. */
void wifi_prov_shutdown(void);

/* Values written to the wifi_control characteristic. */
enum monocle_wifi_command {
    MONOCLE_WIFI_CMD_DOWN = 0,
    MONOCLE_WIFI_CMD_UP   = 1,
};

/* Queues a power change on a worker task. Called from the GATT write handler,
 * which must not block the BLE host task inside esp_wifi_stop(). */
void wifi_prov_request(bool bring_up);

/*
 * Wi-Fi data plane.
 *
 * The monocle listens; the desktop app connects. Every message is
 *
 *     [len: u32 big-endian][type: u8][payload ...]
 *
 * where len counts the type byte plus the payload, so it is always >= 1.
 * Keep in sync with src-tauri/src/socket.rs and docs/protocol.md.
 */
#define MONOCLE_TCP_PORT            3333
#define MONOCLE_FRAME_HEADER_LEN    5
#define MONOCLE_MAX_PAYLOAD         8192

/* How long the data plane stays up with no client and no traffic before the
 * radio is powered down. */
#define MONOCLE_IDLE_TIMEOUT_MS     30000

enum monocle_frame_type {
    MONOCLE_FRAME_ECHO_REQ  = 1,  /* app -> device, payload echoed back */
    MONOCLE_FRAME_ECHO_RESP = 2,  /* device -> app */
    MONOCLE_FRAME_BULK_REQ  = 3,  /* app -> device, u32 BE byte count */
    MONOCLE_FRAME_BULK_DATA = 4,  /* device -> app, synthetic payload */
    MONOCLE_FRAME_BULK_END  = 5,  /* device -> app, u32 BE bytes sent */
};

void tcp_server_start(void);
void tcp_server_stop(void);
#ifdef __cplusplus
}
#endif

#endif
