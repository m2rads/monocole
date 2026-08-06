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

/* Implemented in main.c; called by the GATT write handler once credentials
 * have been received and validated. Returns 0 on success. */
int wifi_prov_connect(const char *ssid, const char *pass);
#ifdef __cplusplus
}
#endif

#endif
