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

#include "esp_log.h"
#include "nvs_flash.h"
#include "nvs.h"
/* BLE */
#include "nimble/nimble_port.h"
#include "nimble/nimble_port_freertos.h"
#include "host/ble_hs.h"
#include "host/util/util.h"
#include "console/console.h"
#include "services/gap/ble_svc_gap.h"
#include "bleprph.h"

/* WIFI */
#include <stdlib.h>
#include <string.h>
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/event_groups.h"
#include "esp_system.h"
#include "esp_wifi.h"
#include "esp_event.h"
#include "esp_log.h"
#include "nvs_flash.h"

#include "lwip/err.h"
#include "lwip/sys.h"
#include "lwip/inet.h"
#include "lwip/netdb.h"
#include "lwip/sockets.h"

#define EXAMPLE_ESP_MAXIMUM_RETRY  CONFIG_EXAMPLE_ESP_MAXIMUM_RETRY

/* The SSID the example ships with. Treated as "unset" so an untouched
 * menuconfig doesn't send us chasing a network that doesn't exist. */
#define EXAMPLE_PLACEHOLDER_SSID   "myssid"

/* Where remembered credentials live, so a reboot doesn't need the app. */
#define MONOCLE_NVS_NAMESPACE      "monocle"
#define MONOCLE_NVS_KEY_SSID       "wifi_ssid"
#define MONOCLE_NVS_KEY_PASS       "wifi_pass"

static int bleprph_gap_event(struct ble_gap_event *event, void *arg);
static uint8_t own_addr_type;

static const char *TAG = "wifi_prph_coex";

static int s_retry_num = 0;

/* esp_wifi_start() has run, so connect requests can be served. */
static bool s_wifi_started;

/* Set once credentials have been supplied. Until then a disconnect event is
 * not something to retry — we simply have nothing to connect to. */
static bool s_provisioned;

/* The credentials currently being tried. Kept so they can be persisted once
 * they are known to work, and reused for reconnects. */
static char s_ssid[MONOCLE_SSID_MAX_LEN + 1];
static char s_pass[MONOCLE_PASS_MAX_LEN + 1];

/**
 * Persists the working credentials.
 *
 * NOTE(security): NVS is not encrypted unless flash encryption is enabled for
 * the device. Until it is, anyone with physical access and a flash reader can
 * recover this passphrase. Enable flash encryption before shipping hardware.
 */
static void
wifi_creds_save(const char *ssid, const char *pass)
{
    nvs_handle_t handle;
    esp_err_t err;

    err = nvs_open(MONOCLE_NVS_NAMESPACE, NVS_READWRITE, &handle);
    if (err != ESP_OK) {
        ESP_LOGW(TAG, "nvs_open failed; credentials not saved (err=%d)", err);
        return;
    }

    if (nvs_set_str(handle, MONOCLE_NVS_KEY_SSID, ssid) != ESP_OK ||
        nvs_set_str(handle, MONOCLE_NVS_KEY_PASS, pass) != ESP_OK ||
        nvs_commit(handle) != ESP_OK) {
        ESP_LOGW(TAG, "failed to persist credentials");
    } else {
        ESP_LOGI(TAG, "credentials saved for SSID \"%s\"", ssid);
    }

    nvs_close(handle);
}

/**
 * Loads remembered credentials. Returns true when both were present.
 */
static bool
wifi_creds_load(char *ssid, size_t ssid_size, char *pass, size_t pass_size)
{
    nvs_handle_t handle;
    bool loaded = false;

    if (nvs_open(MONOCLE_NVS_NAMESPACE, NVS_READONLY, &handle) != ESP_OK) {
        return false;
    }

    /* nvs_get_str writes the size it used back through the same pointer, so
     * each call needs its own copy of the buffer size. */
    size_t ssid_len = ssid_size;
    size_t pass_len = pass_size;
    loaded = nvs_get_str(handle, MONOCLE_NVS_KEY_SSID, ssid, &ssid_len) == ESP_OK &&
             nvs_get_str(handle, MONOCLE_NVS_KEY_PASS, pass, &pass_len) == ESP_OK;

    nvs_close(handle);
    return loaded;
}

static void event_handler(void* arg, esp_event_base_t event_base,
                                int32_t event_id, void* event_data)
{
    if (event_base == WIFI_EVENT && event_id == WIFI_EVENT_STA_START) {
        /* Deliberately does not connect. The radio is up but idle until the
         * app provisions a network over BLE (or stored credentials load). */
        ESP_LOGI(TAG, "wifi station started; idle until provisioned");
    } else if (event_base == WIFI_EVENT && event_id == WIFI_EVENT_STA_DISCONNECTED) {
        wifi_event_sta_disconnected_t *disc =
            (wifi_event_sta_disconnected_t *) event_data;

        if (!s_provisioned) {
            return;
        }

        if (s_retry_num < EXAMPLE_ESP_MAXIMUM_RETRY) {
            s_retry_num++;
            ESP_LOGI(TAG, "connect failed (reason=%d); retry %d/%d",
                     disc->reason, s_retry_num, EXAMPLE_ESP_MAXIMUM_RETRY);
            esp_wifi_connect();
        } else {
            uint8_t reason = disc->reason;
            ESP_LOGW(TAG, "giving up on SSID \"%s\"; reason=%d",
                     s_ssid, disc->reason);
            s_provisioned = false;
            gatt_svr_notify_wifi_state(MONOCLE_WIFI_FAILED, &reason,
                                       sizeof reason);
        }
    } else if (event_base == IP_EVENT && event_id == IP_EVENT_STA_GOT_IP) {
        ip_event_got_ip_t* event = (ip_event_got_ip_t*) event_data;
        uint8_t octets[4] = {
            (uint8_t) esp_ip4_addr1_16(&event->ip_info.ip),
            (uint8_t) esp_ip4_addr2_16(&event->ip_info.ip),
            (uint8_t) esp_ip4_addr3_16(&event->ip_info.ip),
            (uint8_t) esp_ip4_addr4_16(&event->ip_info.ip),
        };

        ESP_LOGI(TAG, "got ip:" IPSTR, IP2STR(&event->ip_info.ip));
        s_retry_num = 0;

        /* Only persist credentials that actually worked. */
        wifi_creds_save(s_ssid, s_pass);

        gatt_svr_notify_wifi_state(MONOCLE_WIFI_CONNECTED, octets,
                                   sizeof octets);

        /* The data plane only exists while we have an address. */
        tcp_server_start();
    }
}

/**
 * Brings the Wi-Fi driver up without connecting to anything.
 *
 * Unlike the stock example this returns immediately, so BLE can start
 * advertising and accept the credentials that decide what we join.
 */
static void wifi_prov_init(void)
{
    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    esp_netif_create_default_wifi_sta();

    wifi_init_config_t cfg = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&cfg));

    ESP_ERROR_CHECK(esp_event_handler_instance_register(WIFI_EVENT,
                                                        ESP_EVENT_ANY_ID,
                                                        &event_handler,
                                                        NULL,
                                                        NULL));
    ESP_ERROR_CHECK(esp_event_handler_instance_register(IP_EVENT,
                                                        IP_EVENT_STA_GOT_IP,
                                                        &event_handler,
                                                        NULL,
                                                        NULL));

    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
    ESP_ERROR_CHECK(esp_wifi_start());
    s_wifi_started = true;

    ESP_LOGI(TAG, "wifi driver started; awaiting credentials over BLE");
}

/**
 * Brings the Wi-Fi driver up if it is currently powered down.
 *
 * Blocks inside esp_wifi_start(), so never call this — or anything reaching
 * it — from the BLE host task.
 */
static int wifi_ensure_started(void)
{
    esp_err_t err;

    if (s_wifi_started) {
        return 0;
    }

    err = esp_wifi_start();
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "esp_wifi_start failed; err=%d", err);
        return -1;
    }
    s_wifi_started = true;
    return 0;
}

/**
 * Joins the given network, powering the radio up first if the idle timer had
 * taken it down. Called at boot for remembered credentials, and from the
 * worker task that services a wifi_creds write.
 *
 * Returns 0 if the attempt was started; progress arrives asynchronously as
 * wifi_state notifications.
 */
int wifi_prov_connect(const char *ssid, const char *pass)
{
    wifi_config_t wifi_config = { 0 };
    esp_err_t err;

    /* Provisioning must work regardless of what the power model is doing.
     * Refusing here is what made every credentials write after an idle
     * teardown fail with a bare ATT "unlikely error". */
    if (wifi_ensure_started() != 0) {
        return -1;
    }

    /* Both strings are already length-validated by the caller; the config
     * fields are fixed-size, and esp_wifi accepts them unterminated when
     * full. */
    strncpy((char *) wifi_config.sta.ssid, ssid, sizeof wifi_config.sta.ssid);
    strncpy((char *) wifi_config.sta.password, pass,
            sizeof wifi_config.sta.password);

    /* An empty passphrase means an open network. Demanding WPA2 there would
     * fail to match any AP at all. */
    wifi_config.sta.threshold.authmode =
        (pass[0] == '\0') ? WIFI_AUTH_OPEN : WIFI_AUTH_WPA2_PSK;

    /* Keep our own copy for persistence and reconnects. */
    strlcpy(s_ssid, ssid, sizeof s_ssid);
    strlcpy(s_pass, pass, sizeof s_pass);

    s_retry_num = 0;
    s_provisioned = true;

    /* Drop any existing association first. This raises a disconnect event,
     * which the handler turns into one retry — harmless, because the new
     * config is already in place by then. */
    esp_wifi_disconnect();

    err = esp_wifi_set_config(WIFI_IF_STA, &wifi_config);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "esp_wifi_set_config failed; err=%d", err);
        s_provisioned = false;
        memset(&wifi_config, 0, sizeof wifi_config);
        return -1;
    }

    ESP_LOGI(TAG, "joining SSID \"%s\"", ssid);
    gatt_svr_notify_wifi_state(MONOCLE_WIFI_CONNECTING, NULL, 0);

    err = esp_wifi_connect();
    if (err != ESP_OK && err != ESP_ERR_WIFI_CONN) {
        ESP_LOGE(TAG, "esp_wifi_connect failed; err=%d", err);
        s_provisioned = false;
        memset(&wifi_config, 0, sizeof wifi_config);
        return -1;
    }

    /* Don't leave a second copy of the passphrase on the stack. */
    memset(&wifi_config, 0, sizeof wifi_config);
    return 0;
}

/**
 * Re-joins using whatever is already in NVS, restarting the radio if it was
 * powered down. This is the "wake the data plane" half of the power model:
 * the app asks for it just before it wants bulk transfer.
 */
int wifi_prov_resume(void)
{
    if (s_ssid[0] == '\0' &&
        !wifi_creds_load(s_ssid, sizeof s_ssid, s_pass, sizeof s_pass)) {
        ESP_LOGW(TAG, "resume requested but no credentials are stored");
        return -1;
    }

    /* wifi_prov_connect() powers the radio up itself. */
    return wifi_prov_connect(s_ssid, s_pass);
}

/**
 * Drops the data plane and powers the Wi-Fi radio down.
 *
 * BLE is untouched — control, status and (later) voice keep working. This is
 * what makes the second radio affordable: it is off except during a burst.
 */
void wifi_prov_shutdown(void)
{
    if (!s_wifi_started) {
        return;
    }

    tcp_server_stop();

    s_provisioned = false;      /* stop the disconnect handler retrying */
    esp_wifi_disconnect();
    esp_wifi_stop();
    s_wifi_started = false;

    ESP_LOGI(TAG, "wifi powered down");
    gatt_svr_notify_wifi_state(MONOCLE_WIFI_IDLE, NULL, 0);
}

static void wifi_cmd_task(void *arg)
{
    if ((uintptr_t) arg != 0) {
        wifi_prov_resume();
    } else {
        wifi_prov_shutdown();
    }
    vTaskDelete(NULL);
}

void wifi_prov_request(bool bring_up)
{
    /* esp_wifi_start/stop can block; the BLE host task must not. */
    xTaskCreate(wifi_cmd_task, "monocle_wifi_cmd", 4096,
                (void *)(uintptr_t)(bring_up ? 1 : 0), 5, NULL);
}

/* Credentials in flight between the GATT write handler and the worker task
 * that acts on them. Heap-allocated per request so a second write cannot
 * overwrite the first one's passphrase mid-join. */
struct wifi_join_request {
    char ssid[MONOCLE_SSID_MAX_LEN + 1];
    char pass[MONOCLE_PASS_MAX_LEN + 1];
};

static void wifi_join_task(void *arg)
{
    struct wifi_join_request *request = arg;

    if (wifi_prov_connect(request->ssid, request->pass) != 0) {
        /* The join never got as far as the air, so there is no 802.11
         * disconnect reason to report. Reason 0 is not a valid one, which
         * makes it a safe marker for "the device failed locally". */
        uint8_t local_failure = 0;
        gatt_svr_notify_wifi_state(MONOCLE_WIFI_FAILED, &local_failure, 1);
    }

    /* Don't leave the passphrase in freed heap. */
    memset(request, 0, sizeof *request);
    free(request);
    vTaskDelete(NULL);
}

void wifi_prov_request_join(const char *ssid, const char *pass)
{
    struct wifi_join_request *request = calloc(1, sizeof *request);

    if (request == NULL) {
        ESP_LOGE(TAG, "out of memory; dropped a provisioning request");
        uint8_t local_failure = 0;
        gatt_svr_notify_wifi_state(MONOCLE_WIFI_FAILED, &local_failure, 1);
        return;
    }

    strlcpy(request->ssid, ssid, sizeof request->ssid);
    strlcpy(request->pass, pass, sizeof request->pass);

    /* wifi_prov_connect() may have to start the radio, which blocks; the BLE
     * host task must not. The ATT response therefore means "accepted", and
     * the outcome arrives as a wifi_state notification. */
    if (xTaskCreate(wifi_join_task, "monocle_wifi_join", 4096, request, 5,
                    NULL) != pdPASS) {
        ESP_LOGE(TAG, "could not start the provisioning task");
        uint8_t local_failure = 0;
        gatt_svr_notify_wifi_state(MONOCLE_WIFI_FAILED, &local_failure, 1);
        memset(request, 0, sizeof *request);
        free(request);
    }
}

void ble_store_config_init(void);

/**
 * Logs information about a connection to the console.
 */
static void
bleprph_print_conn_desc(struct ble_gap_conn_desc *desc)
{
    ESP_LOGI(TAG, "handle=%d our_ota_addr_type=%d our_ota_addr=%02x:%02x:%02x:%02x:%02x:%02x",
             desc->conn_handle, desc->our_ota_addr.type,
             desc->our_ota_addr.val[5],
             desc->our_ota_addr.val[4],
             desc->our_ota_addr.val[3],
             desc->our_ota_addr.val[2],
             desc->our_ota_addr.val[1],
             desc->our_ota_addr.val[0]);

    ESP_LOGI(TAG, "our_id_addr_type=%d our_id_addr=%02x:%02x:%02x:%02x:%02x:%02x",
             desc->our_id_addr.type,
             desc->our_id_addr.val[5],
             desc->our_id_addr.val[4],
             desc->our_id_addr.val[3],
             desc->our_id_addr.val[2],
             desc->our_id_addr.val[1],
             desc->our_id_addr.val[0]);

    ESP_LOGI(TAG, "peer_ota_addr_type=%d peer_ota_addr=%02x:%02x:%02x:%02x:%02x:%02x",
             desc->peer_ota_addr.type,
             desc->peer_ota_addr.val[5],
             desc->peer_ota_addr.val[4],
             desc->peer_ota_addr.val[3],
             desc->peer_ota_addr.val[2],
             desc->peer_ota_addr.val[1],
             desc->peer_ota_addr.val[0]);

    ESP_LOGI(TAG, "peer_id_addr_type=%d peer_id_addr=%02x:%02x:%02x:%02x:%02x:%02x",
             desc->peer_id_addr.type,
             desc->peer_id_addr.val[5],
             desc->peer_id_addr.val[4],
             desc->peer_id_addr.val[3],
             desc->peer_id_addr.val[2],
             desc->peer_id_addr.val[1],
             desc->peer_id_addr.val[0]);

    ESP_LOGI(TAG, "conn_itvl=%d conn_latency=%d supervision_timeout=%d "
                "encrypted=%d authenticated=%d bonded=%d",
                desc->conn_itvl, desc->conn_latency,
                desc->supervision_timeout,
                desc->sec_state.encrypted,
                desc->sec_state.authenticated,
                desc->sec_state.bonded);
}

/**
 * Enables advertising with the following parameters:
 *     o General discoverable mode.
 *     o Undirected connectable mode.
 */
static void
bleprph_advertise(void)
{
    struct ble_gap_adv_params adv_params;
    struct ble_hs_adv_fields fields;
    int rc;

    /**
     *  Set the advertisement data included in our advertisements:
     *     o Flags (indicates advertisement type and other general info).
     *     o Advertising tx power.
     *     o Device name.
     *     o 16-bit service UUIDs (alert notifications).
     */

    memset(&fields, 0, sizeof fields);

    /* Advertise two flags:
     *     o Discoverability in forthcoming advertisement (general)
     *     o BLE-only (BR/EDR unsupported).
     */
    fields.flags = BLE_HS_ADV_F_DISC_GEN |
                   BLE_HS_ADV_F_BREDR_UNSUP;

    /* Indicate that the TX power level field should be included; have the
     * stack fill this value automatically.  This is done by assigning the
     * special value BLE_HS_ADV_TX_PWR_LVL_AUTO.
     */
    fields.tx_pwr_lvl_is_present = 1;
    fields.tx_pwr_lvl = BLE_HS_ADV_TX_PWR_LVL_AUTO;

    const char *name;
    name = ble_svc_gap_device_name();
    fields.name = (uint8_t *)name;
    fields.name_len = strlen(name);
    fields.name_is_complete = 1;

    fields.uuids16 = (ble_uuid16_t[]) {
        BLE_UUID16_INIT(GATT_SVR_SVC_ALERT_UUID)
    };
    fields.num_uuids16 = 1;
    fields.uuids16_is_complete = 1;

    rc = ble_gap_adv_set_fields(&fields);
    if (rc != 0) {
        ESP_LOGE(TAG, "error setting advertisement data; rc=%d", rc);
        return;
    }

    /* Begin advertising. */
    memset(&adv_params, 0, sizeof adv_params);
    adv_params.conn_mode = BLE_GAP_CONN_MODE_UND;
    adv_params.disc_mode = BLE_GAP_DISC_MODE_GEN;
    rc = ble_gap_adv_start(own_addr_type, NULL, BLE_HS_FOREVER,
                           &adv_params, bleprph_gap_event, NULL);
    if (rc != 0) {
        ESP_LOGE(TAG, "error enabling advertisement; rc=%d", rc);
        return;
    }
}

/**
 * The nimble host executes this callback when a GAP event occurs.  The
 * application associates a GAP event callback with each connection that forms.
 * bleprph uses the same callback for all connections.
 *
 * @param event                 The type of event being signalled.
 * @param ctxt                  Various information pertaining to the event.
 * @param arg                   Application-specified argument; unused by
 *                                  bleprph.
 *
 * @return                      0 if the application successfully handled the
 *                                  event; nonzero on failure.  The semantics
 *                                  of the return code is specific to the
 *                                  particular GAP event being signalled.
 */
static int
bleprph_gap_event(struct ble_gap_event *event, void *arg)
{
    struct ble_gap_conn_desc desc;
    int rc;

    switch (event->type) {
    case BLE_GAP_EVENT_CONNECT:
        /* A new connection was established or a connection attempt failed. */
        ESP_LOGI(TAG, "connection %s; status=%d ",
                    event->connect.status == 0 ? "established" : "failed",
                    event->connect.status);
        if (event->connect.status == 0) {
            rc = ble_gap_conn_find(event->connect.conn_handle, &desc);
            assert(rc == 0);
            bleprph_print_conn_desc(&desc);
            gatt_svr_on_connect(event->connect.conn_handle);
        }

        if (event->connect.status != 0) {
            /* Connection failed; resume advertising. */
            bleprph_advertise();
        }
        return 0;

    case BLE_GAP_EVENT_DISCONNECT:
        ESP_LOGI(TAG, "disconnect; reason=%d ", event->disconnect.reason);
        bleprph_print_conn_desc(&event->disconnect.conn);
        gatt_svr_on_disconnect();

        /* Connection terminated; resume advertising. */
        bleprph_advertise();
        return 0;

    case BLE_GAP_EVENT_CONN_UPDATE:
        /* The central has updated the connection parameters. */
        ESP_LOGI(TAG, "connection updated; status=%d ",
                    event->conn_update.status);
        rc = ble_gap_conn_find(event->conn_update.conn_handle, &desc);
        assert(rc == 0);
        bleprph_print_conn_desc(&desc);
        return 0;

    case BLE_GAP_EVENT_ADV_COMPLETE:
        ESP_LOGI(TAG, "advertise complete; reason=%d",
                    event->adv_complete.reason);
        bleprph_advertise();
        return 0;

    case BLE_GAP_EVENT_ENC_CHANGE:
        /* Encryption has been enabled or disabled for this connection. */
        ESP_LOGI(TAG, "encryption change event; status=%d ",
                    event->enc_change.status);
        rc = ble_gap_conn_find(event->enc_change.conn_handle, &desc);
        assert(rc == 0);
        bleprph_print_conn_desc(&desc);
        return 0;

    case BLE_GAP_EVENT_SUBSCRIBE:
        ESP_LOGI(TAG, "subscribe event; conn_handle=%d attr_handle=%d "
                    "reason=%d prevn=%d curn=%d previ=%d curi=%d",
                    event->subscribe.conn_handle,
                    event->subscribe.attr_handle,
                    event->subscribe.reason,
                    event->subscribe.prev_notify,
                    event->subscribe.cur_notify,
                    event->subscribe.prev_indicate,
                    event->subscribe.cur_indicate);
        gatt_svr_on_subscribe(event->subscribe.conn_handle,
                              event->subscribe.attr_handle,
                              event->subscribe.cur_notify);
        return 0;

    case BLE_GAP_EVENT_MTU:
        ESP_LOGI(TAG, "mtu update event; conn_handle=%d cid=%d mtu=%d",
                    event->mtu.conn_handle,
                    event->mtu.channel_id,
                    event->mtu.value);
        return 0;

    case BLE_GAP_EVENT_REPEAT_PAIRING:
        /* We already have a bond with the peer, but it is attempting to
         * establish a new secure link.  This app sacrifices security for
         * convenience: just throw away the old bond and accept the new link.
         */

        /* Delete the old bond. */
        rc = ble_gap_conn_find(event->repeat_pairing.conn_handle, &desc);
        assert(rc == 0);
        ble_store_util_delete_peer(&desc.peer_id_addr);

        /* Return BLE_GAP_REPEAT_PAIRING_RETRY to indicate that the host should
         * continue with the pairing operation.
         */
        return BLE_GAP_REPEAT_PAIRING_RETRY;
    }

    return 0;
}

static void
bleprph_on_reset(int reason)
{
    ESP_LOGE(TAG, "Resetting state; reason=%d", reason);
}

static void
bleprph_on_sync(void)
{
    int rc;

    rc = ble_hs_util_ensure_addr(0);
    assert(rc == 0);

    /* Figure out address to use while advertising (no privacy for now) */
    rc = ble_hs_id_infer_auto(0, &own_addr_type);
    if (rc != 0) {
        ESP_LOGE(TAG, "error determining address type; rc=%d", rc);
        return;
    }

    /* Printing ADDR */
    uint8_t addr_val[6] = {0};
    rc = ble_hs_id_copy_addr(own_addr_type, addr_val, NULL);

    ESP_LOGI(TAG, "Device Address:%02x:%02x:%02x:%02x:%02x:%02x",
             addr_val[5],
             addr_val[4],
             addr_val[3],
             addr_val[2],
             addr_val[1],
             addr_val[0]);
    /* Begin advertising. */
    bleprph_advertise();
}

void bleprph_host_task(void *param)
{
    ESP_LOGI(TAG, "BLE Host Task Started");
    /* This function will return only when nimble_port_stop() is executed */
    nimble_port_run();

    nimble_port_freertos_deinit();
}

void
app_main(void)
{
    /* Initialize NVS — it is used to store PHY calibration data */
    esp_err_t ret = nvs_flash_init();
    if (ret == ESP_ERR_NVS_NO_FREE_PAGES || ret == ESP_ERR_NVS_NEW_VERSION_FOUND) {
        ESP_ERROR_CHECK(nvs_flash_erase());
        ret = nvs_flash_init();
    }
    ESP_ERROR_CHECK(ret);

    /* Bring the radio up but stay unassociated: which network we join is the
     * app's decision, delivered over BLE. */
    wifi_prov_init();

    ret = nimble_port_init();
    if (ret != ESP_OK) {
        ESP_LOGE(TAG, "Failed to init nimble %d ", ret);
        return;
    }
    /* Initialize the NimBLE host configuration. */
    ble_hs_cfg.reset_cb = bleprph_on_reset;
    ble_hs_cfg.sync_cb = bleprph_on_sync;
    ble_hs_cfg.gatts_register_cb = gatt_svr_register_cb;
    ble_hs_cfg.store_status_cb = ble_store_util_status_rr;

    /* Security. wifi_creds and wifi_control are WRITE_ENC, so the host will
     * not dispatch a write to us until the link is encrypted — which requires
     * pairing, and (to survive a reboot) bonding.
     *
     * Distributing the LTK in both directions is what lets the app reconnect
     * silently instead of re-pairing every session; the keys are persisted by
     * ble_store_config_init() below, which needs CONFIG_BT_NIMBLE_NVS_PERSIST.
     * Without persistence the chip forgets the bond on every boot while the
     * Mac keeps believing it is paired, and every encrypted write then fails
     * with a bare ATT "unlikely error". See docs/protocol.md, Security.
     *
     * No I/O capability: the monocle has no keypad or display, so pairing is
     * "just works". That is unauthenticated — fine for now, worth revisiting
     * before shipping, since it leaves the passphrase open to an active
     * man-in-the-middle at pairing time. */
    ble_hs_cfg.sm_io_cap = BLE_HS_IO_NO_INPUT_OUTPUT;
    ble_hs_cfg.sm_bonding = 1;
    ble_hs_cfg.sm_sc = 1;
    ble_hs_cfg.sm_mitm = 0;
    ble_hs_cfg.sm_our_key_dist |= BLE_SM_PAIR_KEY_DIST_ENC;
    ble_hs_cfg.sm_their_key_dist |= BLE_SM_PAIR_KEY_DIST_ENC;

#if MYNEWT_VAL(BLE_GATTS)
    int rc;
    rc = gatt_svr_init();
    assert(rc == 0);

    /* Set the default device name. */
    rc = ble_svc_gap_device_name_set("nimble-bleprph");
    assert(rc == 0);
#endif

    /* XXX Need to have template for store */
    ble_store_config_init();

    nimble_port_freertos_init(bleprph_host_task);

    /* If this device has been provisioned before, reconnect without waiting
     * for the app. Notifications sent before a client subscribes are dropped,
     * which is fine — the app reads the state when it connects. */
    if (wifi_creds_load(s_ssid, sizeof s_ssid, s_pass, sizeof s_pass)) {
        ESP_LOGI(TAG, "using stored credentials for SSID \"%s\"", s_ssid);
        wifi_prov_connect(s_ssid, s_pass);
    } else if (strcmp(CONFIG_EXAMPLE_ESP_WIFI_SSID, EXAMPLE_PLACEHOLDER_SSID) != 0) {
        /* Development convenience: credentials set in menuconfig are used when
         * nothing has been provisioned over BLE yet. A successful join saves
         * them to NVS, after which the stored copy wins and this is skipped.
         * Leave the SSID at its placeholder to disable this path. */
        ESP_LOGI(TAG, "using menuconfig credentials for SSID \"%s\"",
                 CONFIG_EXAMPLE_ESP_WIFI_SSID);
        wifi_prov_connect(CONFIG_EXAMPLE_ESP_WIFI_SSID,
                          CONFIG_EXAMPLE_ESP_WIFI_PASSWORD);
    } else {
        ESP_LOGI(TAG, "no stored or configured credentials; "
                      "write them to the wifi_creds characteristic");
    }
}
