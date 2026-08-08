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

#include <assert.h>
#include <stdio.h>
#include <string.h>
#include "host/ble_hs.h"
#include "host/ble_uuid.h"
#include "services/gap/ble_svc_gap.h"
#include "services/gatt/ble_svc_gatt.h"
#include "bleprph.h"
#include "services/ans/ble_svc_ans.h"
#include "esp_log.h"
#include "display.h"


/**
 * The vendor specific security test service consists of two characteristics:
 *     o random-number-generator: generates a random 32-bit number each time
 *       it is read.
 *     o static-value: a single-byte characteristic that can always be read.
 */

/* 59462f12-9543-9999-12c8-58b459a2712d */
static const ble_uuid128_t gatt_svr_svc_sec_test_uuid =
    BLE_UUID128_INIT(0x2d, 0x71, 0xa2, 0x59, 0xb4, 0x58, 0xc8, 0x12,
                     0x99, 0x99, 0x43, 0x95, 0x12, 0x2f, 0x46, 0x59);

/* 5c3a659e-897e-45e1-b016-007107c96df6 */
static const ble_uuid128_t gatt_svr_chr_sec_test_rand_uuid =
    BLE_UUID128_INIT(0xf6, 0x6d, 0xc9, 0x07, 0x71, 0x00, 0x16, 0xb0,
                     0xe1, 0x45, 0x7e, 0x89, 0x9e, 0x65, 0x3a, 0x5c);

/* 5c3a659e-897e-45e1-b016-007107c96df7 */
static const ble_uuid128_t gatt_svr_chr_sec_test_static_uuid =
    BLE_UUID128_INIT(0xf7, 0x6d, 0xc9, 0x07, 0x71, 0x00, 0x16, 0xb0,
                     0xe1, 0x45, 0x7e, 0x89, 0x9e, 0x65, 0x3a, 0x5c);

/*
 * Monocle Wi-Fi provisioning service.
 *
 * The desktop app writes the credentials of the network it wants the monocle
 * to join, and the monocle reports its join progress (and, on success, the
 * address the app should open a socket to) back over a notification.
 *
 * These UUIDs are the contract with src-tauri/src/ble.rs — see
 * docs/protocol.md. BLE_UUID128_INIT takes bytes in wire order, which is the
 * reverse of how the UUID is written in the comment above each one.
 */

/* 83486508-636c-4260-9119-c0ccc2004219 */
static const ble_uuid128_t gatt_svr_svc_monocle_uuid =
    BLE_UUID128_INIT(0x19, 0x42, 0x00, 0xc2, 0xcc, 0xc0, 0x19, 0x91,
                     0x60, 0x42, 0x6c, 0x63, 0x08, 0x65, 0x48, 0x83);

/* 2c9b4a45-d3a5-4bf9-ac60-1f5f2e98db3c — app writes credentials here. */
static const ble_uuid128_t gatt_svr_chr_wifi_creds_uuid =
    BLE_UUID128_INIT(0x3c, 0xdb, 0x98, 0x2e, 0x5f, 0x1f, 0x60, 0xac,
                     0xf9, 0x4b, 0xa5, 0xd3, 0x45, 0x4a, 0x9b, 0x2c);

/* 1ad1e743-dcae-422d-a7a8-68b4d695ac8b — device notifies join state here. */
static const ble_uuid128_t gatt_svr_chr_wifi_state_uuid =
    BLE_UUID128_INIT(0x8b, 0xac, 0x95, 0xd6, 0xb4, 0x68, 0xa8, 0xa7,
                     0x2d, 0x42, 0xae, 0xdc, 0x43, 0xe7, 0xd1, 0x1a);

/* e4782756-b76f-482c-9a0a-8c546a9134f1 — app powers the data plane up/down. */
static const ble_uuid128_t gatt_svr_chr_wifi_control_uuid =
    BLE_UUID128_INIT(0xf1, 0x34, 0x91, 0x6a, 0x54, 0x8c, 0x0a, 0x9a,
                     0x2c, 0x48, 0x6f, 0xb7, 0x56, 0x27, 0x78, 0xe4);

/* e474939e-3010-4284-b280-4f365b6fe723 — what the wearer sees on the panel. */
static const ble_uuid128_t gatt_svr_chr_display_uuid =
    BLE_UUID128_INIT(0x23, 0xe7, 0x6f, 0x5b, 0x36, 0x4f, 0x80, 0xb2,
                     0x84, 0x42, 0x10, 0x30, 0x9e, 0x93, 0x74, 0xe4);

static const char* TAG = "wifi_prph_coex";

static uint8_t gatt_svr_sec_test_static_val;

/* NimBLE fills this in during registration; notifications are addressed to
 * the value handle, not the declaration handle. */
static uint16_t gatt_svr_chr_wifi_state_handle;

/* The connection we notify on, and whether its client has subscribed.
 * BLE_HS_CONN_HANDLE_NONE means "not connected". */
static uint16_t gatt_svr_conn_handle = BLE_HS_CONN_HANDLE_NONE;
static bool gatt_svr_wifi_state_subscribed;

static int
gatt_svr_chr_access_sec_test(uint16_t conn_handle, uint16_t attr_handle,
                             struct ble_gatt_access_ctxt *ctxt,
                             void *arg);

static int
gatt_svr_chr_access_wifi(uint16_t conn_handle, uint16_t attr_handle,
                         struct ble_gatt_access_ctxt *ctxt,
                         void *arg);

static int
gatt_svr_chr_access_display(uint16_t conn_handle, uint16_t attr_handle,
                            struct ble_gatt_access_ctxt *ctxt,
                            void *arg);

static const struct ble_gatt_svc_def gatt_svr_svcs[] = {
    {
        /*** Service: Security test. */
        .type = BLE_GATT_SVC_TYPE_PRIMARY,
        .uuid = &gatt_svr_svc_sec_test_uuid.u,
        .characteristics = (struct ble_gatt_chr_def[])
        { {
                /*** Characteristic: Random number generator. */
                .uuid = &gatt_svr_chr_sec_test_rand_uuid.u,
                .access_cb = gatt_svr_chr_access_sec_test,
                .flags = BLE_GATT_CHR_F_READ
            }, {
                /*** Characteristic: Static value. */
                .uuid = &gatt_svr_chr_sec_test_static_uuid.u,
                .access_cb = gatt_svr_chr_access_sec_test,
                .flags = BLE_GATT_CHR_F_READ |
                BLE_GATT_CHR_F_WRITE
            }, {
                0, /* No more characteristics in this service. */
            }
        },
    },

    {
        /*** Service: Monocle Wi-Fi provisioning. */
        .type = BLE_GATT_SVC_TYPE_PRIMARY,
        .uuid = &gatt_svr_svc_monocle_uuid.u,
        .characteristics = (struct ble_gatt_chr_def[])
        { {
                /*** Characteristic: Wi-Fi credentials (app -> device).
                 *
                 * WRITE_ENC is what keeps the passphrase off the air in
                 * cleartext: NimBLE refuses the write, and starts pairing,
                 * unless the link is already encrypted. It depends on bonding
                 * being enabled (CONFIG_EXAMPLE_BONDING).
                 */
                .uuid = &gatt_svr_chr_wifi_creds_uuid.u,
                .access_cb = gatt_svr_chr_access_wifi,
                .flags = BLE_GATT_CHR_F_WRITE |
                BLE_GATT_CHR_F_WRITE_ENC
            }, {
                /*** Characteristic: Wi-Fi join state (device -> app). */
                .uuid = &gatt_svr_chr_wifi_state_uuid.u,
                .access_cb = gatt_svr_chr_access_wifi,
                .flags = BLE_GATT_CHR_F_NOTIFY,
                .val_handle = &gatt_svr_chr_wifi_state_handle
            }, {
                /*** Characteristic: data-plane power control (app -> device).
                 *
                 * Encrypted for the same reason as the credentials: an
                 * unauthenticated peer should not be able to flatten the
                 * battery by cycling the Wi-Fi radio.
                 */
                .uuid = &gatt_svr_chr_wifi_control_uuid.u,
                .access_cb = gatt_svr_chr_access_wifi,
                .flags = BLE_GATT_CHR_F_WRITE |
                BLE_GATT_CHR_F_WRITE_ENC
            }, {
                /*** Characteristic: panel contents (app -> device).
                 *
                 * Encrypted like the rest: an unauthenticated peer should not
                 * be able to put text in front of the wearer's eye.
                 */
                .uuid = &gatt_svr_chr_display_uuid.u,
                .access_cb = gatt_svr_chr_access_display,
                .flags = BLE_GATT_CHR_F_WRITE |
                BLE_GATT_CHR_F_WRITE_ENC
            }, {
                0, /* No more characteristics in this service. */
            }
        },
    },

    {
        0, /* No more services. */
    },
};

static int
gatt_svr_chr_write(struct os_mbuf *om, uint16_t min_len, uint16_t max_len,
                   void *dst, uint16_t *len)
{
    uint16_t om_len;
    int rc;

    om_len = OS_MBUF_PKTLEN(om);
    if (om_len < min_len || om_len > max_len) {
        return BLE_ATT_ERR_INVALID_ATTR_VALUE_LEN;
    }

    rc = ble_hs_mbuf_to_flat(om, dst, max_len, len);
    if (rc != 0) {
        return BLE_ATT_ERR_UNLIKELY;
    }

    return 0;
}

static int
gatt_svr_chr_access_sec_test(uint16_t conn_handle, uint16_t attr_handle,
                             struct ble_gatt_access_ctxt *ctxt,
                             void *arg)
{
    const ble_uuid_t *uuid;
    int rand_num;
    int rc;

    uuid = ctxt->chr->uuid;

    /* Determine which characteristic is being accessed by examining its
     * 128-bit UUID.
     */

    if (ble_uuid_cmp(uuid, &gatt_svr_chr_sec_test_rand_uuid.u) == 0) {
        assert(ctxt->op == BLE_GATT_ACCESS_OP_READ_CHR);

        /* Respond with a 32-bit random number. */
        rand_num = rand();
        rc = os_mbuf_append(ctxt->om, &rand_num, sizeof rand_num);
        return rc == 0 ? 0 : BLE_ATT_ERR_INSUFFICIENT_RES;
    }

    if (ble_uuid_cmp(uuid, &gatt_svr_chr_sec_test_static_uuid.u) == 0) {
        switch (ctxt->op) {
        case BLE_GATT_ACCESS_OP_READ_CHR:
            rc = os_mbuf_append(ctxt->om, &gatt_svr_sec_test_static_val,
                                sizeof gatt_svr_sec_test_static_val);
            return rc == 0 ? 0 : BLE_ATT_ERR_INSUFFICIENT_RES;

        case BLE_GATT_ACCESS_OP_WRITE_CHR:
            rc = gatt_svr_chr_write(ctxt->om,
                                    sizeof gatt_svr_sec_test_static_val,
                                    sizeof gatt_svr_sec_test_static_val,
                                    &gatt_svr_sec_test_static_val, NULL);
            return rc;

        default:
            assert(0);
            return BLE_ATT_ERR_UNLIKELY;
        }
    }

    /* Unknown characteristic; the nimble stack should not have called this
     * function.
     */
    assert(0);
    return BLE_ATT_ERR_UNLIKELY;
}

/**
 * Handles access to the Wi-Fi provisioning characteristics.
 *
 * Only wifi_creds is ever reached: wifi_state is notify-only, so the stack
 * has nothing to ask us about it.
 *
 * The write payload is [ssid_len:u8][ssid][pass_len:u8][pass]. Worst case is
 * 97 bytes, which fits in a single ATT write at any MTU macOS negotiates, so
 * there is no reassembly to do here.
 */
static int
gatt_svr_chr_access_wifi(uint16_t conn_handle, uint16_t attr_handle,
                         struct ble_gatt_access_ctxt *ctxt,
                         void *arg)
{
    uint8_t buf[1 + MONOCLE_SSID_MAX_LEN + 1 + MONOCLE_PASS_MAX_LEN];
    char ssid[MONOCLE_SSID_MAX_LEN + 1];
    char pass[MONOCLE_PASS_MAX_LEN + 1];
    struct ble_gap_conn_desc desc;
    uint8_t ssid_len, pass_len;
    uint16_t len;
    int rc;

    if (ctxt->op != BLE_GATT_ACCESS_OP_WRITE_CHR) {
        return BLE_ATT_ERR_UNLIKELY;
    }

    /* Defence in depth. BLE_GATT_CHR_F_WRITE_ENC should already guarantee an
     * encrypted link, but a Wi-Fi passphrase is worth confirming rather than
     * trusting a single flag in a table far from here. */
    rc = ble_gap_conn_find(conn_handle, &desc);
    if (rc != 0 || !desc.sec_state.encrypted) {
        ESP_LOGW(TAG, "rejected write on an unencrypted link");
        return BLE_ATT_ERR_INSUFFICIENT_ENC;
    }

    if (ble_uuid_cmp(ctxt->chr->uuid, &gatt_svr_chr_wifi_control_uuid.u) == 0) {
        uint8_t command;

        rc = gatt_svr_chr_write(ctxt->om, 1, 1, &command, NULL);
        if (rc != 0) {
            return rc;
        }
        if (command != MONOCLE_WIFI_CMD_UP && command != MONOCLE_WIFI_CMD_DOWN) {
            return BLE_ATT_ERR_INVALID_ATTR_VALUE_LEN;
        }

        ESP_LOGI(TAG, "wifi control: %s",
                 command == MONOCLE_WIFI_CMD_UP ? "up" : "down");
        wifi_prov_request(command == MONOCLE_WIFI_CMD_UP);
        return 0;
    }

    if (ble_uuid_cmp(ctxt->chr->uuid, &gatt_svr_chr_wifi_creds_uuid.u) != 0) {
        return BLE_ATT_ERR_UNLIKELY;
    }

    /* Shortest legal message is a 1-byte SSID with an empty passphrase. */
    rc = gatt_svr_chr_write(ctxt->om, 3, sizeof buf, buf, &len);
    if (rc != 0) {
        return rc;
    }

    /* Every offset below is computed from bytes a remote device chose, so each
     * is checked against the length actually received before being used. */
    ssid_len = buf[0];
    if (ssid_len == 0 || ssid_len > MONOCLE_SSID_MAX_LEN ||
        len < 2 + ssid_len) {
        return BLE_ATT_ERR_INVALID_ATTR_VALUE_LEN;
    }

    pass_len = buf[1 + ssid_len];
    if (pass_len > MONOCLE_PASS_MAX_LEN || len < 2 + ssid_len + pass_len) {
        return BLE_ATT_ERR_INVALID_ATTR_VALUE_LEN;
    }

    memcpy(ssid, &buf[1], ssid_len);
    ssid[ssid_len] = '\0';
    memcpy(pass, &buf[2 + ssid_len], pass_len);
    pass[pass_len] = '\0';

    /* Log the network but never the passphrase. */
    ESP_LOGI(TAG, "credentials received for SSID \"%s\" (%u-byte key)",
             ssid, pass_len);

    /* Hand off rather than join here: this is the BLE host task, and the join
     * may have to start the radio, which blocks. The write is answered as soon
     * as the request is accepted; whether the network was actually joined
     * arrives as a wifi_state notification. */
    wifi_prov_request_join(ssid, pass);

    /* Don't leave the passphrase lying in stack memory after we're done. */
    memset(buf, 0, sizeof buf);
    memset(pass, 0, sizeof pass);

    return 0;
}

/**
 * Handles writes to the display characteristic: `[op: u8][utf-8 text]`.
 *
 * Nothing is drawn here. A full panel update is ~25 ms of I2C and this is the
 * BLE host task, so the payload is queued and the display task renders it.
 */
static int
gatt_svr_chr_access_display(uint16_t conn_handle, uint16_t attr_handle,
                            struct ble_gatt_access_ctxt *ctxt,
                            void *arg)
{
    uint8_t buf[1 + DISPLAY_TEXT_MAX];
    struct ble_gap_conn_desc desc;
    uint16_t len;
    int rc;

    if (ctxt->op != BLE_GATT_ACCESS_OP_WRITE_CHR) {
        return BLE_ATT_ERR_UNLIKELY;
    }

    /* Same defence in depth as the credentials write: the flag in the table
     * should already guarantee this, but confirm it here too. */
    rc = ble_gap_conn_find(conn_handle, &desc);
    if (rc != 0 || !desc.sec_state.encrypted) {
        ESP_LOGW(TAG, "rejected display write on an unencrypted link");
        return BLE_ATT_ERR_INSUFFICIENT_ENC;
    }

    /* At least the op byte; at most one ATT write's worth. */
    rc = gatt_svr_chr_write(ctxt->om, 1, sizeof buf, buf, &len);
    if (rc != 0) {
        return rc;
    }

    uint8_t op = buf[0];
    if (op != DISPLAY_OP_CLEAR && op != DISPLAY_OP_SET &&
        op != DISPLAY_OP_APPEND) {
        ESP_LOGW(TAG, "unknown display op %u", op);
        return BLE_ATT_ERR_INVALID_ATTR_VALUE_LEN;
    }

    ESP_LOGI(TAG, "display write: op=%u, %u bytes", op, (unsigned)(len - 1));

    if (!display_post(op, (const char *)&buf[1], len - 1)) {
        /* The queue is full, so the panel is already behind. Dropping beats
         * blocking the radio, and the next update supersedes this one anyway. */
        ESP_LOGW(TAG, "display queue full; dropped an update");
    }
    return 0;
}

void
gatt_svr_on_connect(uint16_t conn_handle)
{
    gatt_svr_conn_handle = conn_handle;
}

void
gatt_svr_on_disconnect(void)
{
    gatt_svr_conn_handle = BLE_HS_CONN_HANDLE_NONE;
    gatt_svr_wifi_state_subscribed = false;
}

void
gatt_svr_on_subscribe(uint16_t conn_handle, uint16_t attr_handle,
                      int cur_notify)
{
    if (attr_handle == gatt_svr_chr_wifi_state_handle) {
        gatt_svr_conn_handle = conn_handle;
        gatt_svr_wifi_state_subscribed = cur_notify != 0;
    }
}

void
gatt_svr_notify_wifi_state(uint8_t state, const void *extra, uint8_t extra_len)
{
    uint8_t payload[1 + 4];   /* state byte + the widest extra we send (IPv4) */
    struct os_mbuf *om;
    int rc;

    if (gatt_svr_conn_handle == BLE_HS_CONN_HANDLE_NONE ||
        !gatt_svr_wifi_state_subscribed) {
        return;
    }

    if (extra_len > sizeof payload - 1) {
        ESP_LOGE(TAG, "wifi_state extra too long (%u)", extra_len);
        return;
    }

    payload[0] = state;
    if (extra_len > 0) {
        memcpy(&payload[1], extra, extra_len);
    }

    /* ble_gatts_notify_custom consumes the mbuf, including on failure. */
    om = ble_hs_mbuf_from_flat(payload, 1 + extra_len);
    if (om == NULL) {
        ESP_LOGE(TAG, "out of mbufs; dropped wifi_state notification");
        return;
    }

    rc = ble_gatts_notify_custom(gatt_svr_conn_handle,
                                 gatt_svr_chr_wifi_state_handle, om);
    if (rc != 0) {
        ESP_LOGW(TAG, "wifi_state notify failed; rc=%d", rc);
    }
}

void
gatt_svr_register_cb(struct ble_gatt_register_ctxt *ctxt, void *arg)
{
    char buf[BLE_UUID_STR_LEN];

    switch (ctxt->op) {
    case BLE_GATT_REGISTER_OP_SVC:
        ESP_LOGD(TAG, "registered service %s with handle=%d",
                    ble_uuid_to_str(ctxt->svc.svc_def->uuid, buf),
                    ctxt->svc.handle);
        break;

    case BLE_GATT_REGISTER_OP_CHR:
        ESP_LOGD(TAG, "registering characteristic %s with "
                    "def_handle=%d val_handle=%d\n",
                    ble_uuid_to_str(ctxt->chr.chr_def->uuid, buf),
                    ctxt->chr.def_handle,
                    ctxt->chr.val_handle);
        break;

    case BLE_GATT_REGISTER_OP_DSC:
        ESP_LOGD(TAG, "registering descriptor %s with handle=%d",
                    ble_uuid_to_str(ctxt->dsc.dsc_def->uuid, buf),
                    ctxt->dsc.handle);
        break;

    default:
        assert(0);
        break;
    }
}

int
gatt_svr_init(void)
{
    int rc;

#if CONFIG_BT_NIMBLE_GAP_SERVICE
    ble_svc_gap_init();
#endif /* CONFIG_BT_NIMBLE_GAP_SERVICE */
#if MYNEWT_VAL(BLE_GATTS)
    ble_svc_gatt_init();
#endif
#if CONFIG_BT_NIMBLE_ANS_SERVICE
    ble_svc_ans_init();
#endif

    rc = ble_gatts_count_cfg(gatt_svr_svcs);
    if (rc != 0) {
        return rc;
    }

    rc = ble_gatts_add_svcs(gatt_svr_svcs);
    if (rc != 0) {
        return rc;
    }

    return 0;
}
