/*
 * Wi-Fi data plane: a single-client TCP server.
 *
 * Carries the bulk traffic that BLE is bad at — today synthetic bytes for
 * throughput measurement, later JPEG stills. Started when the station gets an
 * address and stopped when the link has been idle, because the whole point of
 * putting images on Wi-Fi is that the radio is off the rest of the time.
 *
 * One client at a time, by design: there is exactly one desktop app.
 */

#include <errno.h>
#include <string.h>

#include <lwip/netdb.h>
#include <lwip/sockets.h>

#include "esp_log.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#include "bleprph.h"

static const char *TAG = "monocle_tcp";

/* Chunk used for bulk sends. Large enough to keep the radio busy, small
 * enough that one lives comfortably on the task stack budget. */
#define BULK_CHUNK_LEN      4096

/* Bound on a single accept()/recv() block, so the idle timer stays responsive
 * without spinning. */
#define SOCKET_TIMEOUT_MS   1000

static TaskHandle_t s_task;
static volatile bool s_running;

static int64_t now_ms(void)
{
    return esp_timer_get_time() / 1000;
}

/**
 * Reads exactly len bytes. Returns 0 on success, -1 if the peer closed or
 * errored, and 1 on timeout (no data, connection still good).
 */
static int read_exact(int sock, void *dst, size_t len)
{
    uint8_t *out = dst;
    size_t got = 0;

    while (got < len) {
        int rc = recv(sock, out + got, len - got, 0);
        if (rc > 0) {
            got += rc;
            continue;
        }
        if (rc == 0) {
            return -1;                       /* orderly shutdown by the peer */
        }
        if (errno == EAGAIN || errno == EWOULDBLOCK) {
            /* A timeout part-way through a frame still means the peer is
             * mid-send; only a timeout at a frame boundary is "idle". */
            if (got == 0) {
                return 1;
            }
            continue;
        }
        ESP_LOGW(TAG, "recv failed; errno=%d", errno);
        return -1;
    }
    return 0;
}

static int write_all(int sock, const void *src, size_t len)
{
    const uint8_t *in = src;
    size_t sent = 0;

    while (sent < len) {
        int rc = send(sock, in + sent, len - sent, 0);
        if (rc > 0) {
            sent += rc;
            continue;
        }
        if (rc < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            continue;                        /* transmit buffer full; retry */
        }
        ESP_LOGW(TAG, "send failed; errno=%d", errno);
        return -1;
    }
    return 0;
}

static void put_u32(uint8_t *dst, uint32_t value)
{
    dst[0] = (uint8_t)(value >> 24);
    dst[1] = (uint8_t)(value >> 16);
    dst[2] = (uint8_t)(value >> 8);
    dst[3] = (uint8_t)value;
}

static uint32_t get_u32(const uint8_t *src)
{
    return ((uint32_t)src[0] << 24) | ((uint32_t)src[1] << 16) |
           ((uint32_t)src[2] << 8) | (uint32_t)src[3];
}

static int send_frame(int sock, uint8_t type, const void *payload,
                      uint32_t payload_len)
{
    uint8_t header[MONOCLE_FRAME_HEADER_LEN];

    put_u32(header, payload_len + 1);        /* len covers the type byte */
    header[4] = type;

    if (write_all(sock, header, sizeof header) != 0) {
        return -1;
    }
    if (payload_len > 0 && write_all(sock, payload, payload_len) != 0) {
        return -1;
    }
    return 0;
}

/**
 * Streams `total` synthetic bytes, then a BULK_END carrying the count.
 *
 * The payload is a repeating pattern rather than zeros so a compressing or
 * deduplicating layer in between cannot flatter the measurement.
 */
static int send_bulk(int sock, uint32_t total)
{
    static uint8_t chunk[BULK_CHUNK_LEN];
    uint8_t trailer[4];
    uint32_t sent = 0;
    int64_t started;

    for (size_t i = 0; i < sizeof chunk; i++) {
        chunk[i] = (uint8_t)i;
    }

    ESP_LOGI(TAG, "bulk transfer requested: %" PRIu32 " bytes", total);
    started = now_ms();

    while (sent < total) {
        uint32_t remaining = total - sent;
        uint32_t n = remaining < BULK_CHUNK_LEN ? remaining : BULK_CHUNK_LEN;

        if (send_frame(sock, MONOCLE_FRAME_BULK_DATA, chunk, n) != 0) {
            return -1;
        }
        sent += n;
    }

    put_u32(trailer, sent);
    if (send_frame(sock, MONOCLE_FRAME_BULK_END, trailer, sizeof trailer) != 0) {
        return -1;
    }

    int64_t elapsed = now_ms() - started;
    if (elapsed <= 0) {
        elapsed = 1;
    }
    ESP_LOGI(TAG, "bulk transfer done: %" PRIu32 " bytes in %lld ms (%lld kbps)",
             sent, elapsed, (long long)((int64_t)sent * 8 / elapsed));
    return 0;
}

/**
 * Serves one connected client until it disconnects or goes idle.
 * Returns true if the link went idle (rather than closing).
 */
static bool serve_client(int sock)
{
    uint8_t header[MONOCLE_FRAME_HEADER_LEN];
    static uint8_t payload[MONOCLE_MAX_PAYLOAD];
    int64_t last_activity = now_ms();

    while (s_running) {
        int rc = read_exact(sock, header, sizeof header);

        if (rc < 0) {
            ESP_LOGI(TAG, "client disconnected");
            return false;
        }
        if (rc > 0) {
            if (now_ms() - last_activity > MONOCLE_IDLE_TIMEOUT_MS) {
                ESP_LOGI(TAG, "client idle for %d ms; closing",
                         MONOCLE_IDLE_TIMEOUT_MS);
                return true;
            }
            continue;
        }

        last_activity = now_ms();

        uint32_t len = get_u32(header);
        uint8_t type = header[4];

        /* len includes the type byte, so a zero-length frame is malformed and
         * an oversized one would overrun the payload buffer. */
        if (len == 0 || len - 1 > sizeof payload) {
            ESP_LOGW(TAG, "bad frame length %" PRIu32 "; dropping client", len);
            return false;
        }

        uint32_t payload_len = len - 1;
        if (payload_len > 0 && read_exact(sock, payload, payload_len) != 0) {
            return false;
        }

        switch (type) {
        case MONOCLE_FRAME_ECHO_REQ:
            if (send_frame(sock, MONOCLE_FRAME_ECHO_RESP, payload,
                           payload_len) != 0) {
                return false;
            }
            break;

        case MONOCLE_FRAME_BULK_REQ:
            if (payload_len != 4) {
                ESP_LOGW(TAG, "bulk request needs 4 bytes, got %" PRIu32,
                         payload_len);
                return false;
            }
            if (send_bulk(sock, get_u32(payload)) != 0) {
                return false;
            }
            last_activity = now_ms();
            break;

        default:
            ESP_LOGW(TAG, "unknown frame type %u; dropping client", type);
            return false;
        }
    }
    return false;
}

static int listen_socket(void)
{
    struct sockaddr_in addr = {
        .sin_family = AF_INET,
        .sin_addr.s_addr = htonl(INADDR_ANY),
        .sin_port = htons(MONOCLE_TCP_PORT),
    };
    struct timeval timeout = {
        .tv_sec = SOCKET_TIMEOUT_MS / 1000,
        .tv_usec = (SOCKET_TIMEOUT_MS % 1000) * 1000,
    };
    int opt = 1;
    int sock;

    sock = socket(AF_INET, SOCK_STREAM, IPPROTO_IP);
    if (sock < 0) {
        ESP_LOGE(TAG, "socket() failed; errno=%d", errno);
        return -1;
    }

    /* Without this a restart within TIME_WAIT fails to bind. */
    setsockopt(sock, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof opt);
    setsockopt(sock, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);

    if (bind(sock, (struct sockaddr *)&addr, sizeof addr) != 0 ||
        listen(sock, 1) != 0) {
        ESP_LOGE(TAG, "bind/listen failed; errno=%d", errno);
        close(sock);
        return -1;
    }

    ESP_LOGI(TAG, "listening on port %d", MONOCLE_TCP_PORT);
    return sock;
}

static void tcp_server_task(void *arg)
{
    int listener = listen_socket();
    int64_t idle_since = now_ms();

    if (listener < 0) {
        s_running = false;
        s_task = NULL;
        vTaskDelete(NULL);
        return;
    }

    while (s_running) {
        struct sockaddr_storage source;
        socklen_t source_len = sizeof source;
        struct timeval timeout = {
            .tv_sec = SOCKET_TIMEOUT_MS / 1000,
            .tv_usec = (SOCKET_TIMEOUT_MS % 1000) * 1000,
        };

        int client = accept(listener, (struct sockaddr *)&source, &source_len);
        if (client < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK) {
                /* Nobody has connected. The radio is up and costing power, so
                 * this is exactly the window the idle timeout exists for. */
                if (now_ms() - idle_since > MONOCLE_IDLE_TIMEOUT_MS) {
                    ESP_LOGI(TAG, "no client for %d ms; powering wifi down",
                             MONOCLE_IDLE_TIMEOUT_MS);
                    break;
                }
                continue;
            }
            ESP_LOGW(TAG, "accept failed; errno=%d", errno);
            continue;
        }

        setsockopt(client, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);

        /* Latency beats coalescing for small echo frames. */
        int nodelay = 1;
        setsockopt(client, IPPROTO_TCP, TCP_NODELAY, &nodelay, sizeof nodelay);

        ESP_LOGI(TAG, "client connected");
        serve_client(client);

        shutdown(client, SHUT_RDWR);
        close(client);
        idle_since = now_ms();
    }

    close(listener);
    ESP_LOGI(TAG, "server stopped");

    s_task = NULL;
    bool power_down = s_running;   /* false when tcp_server_stop() asked us to */
    s_running = false;

    /* Only tear the radio down if we exited on our own idle timer; an explicit
     * stop is already part of a shutdown in progress. */
    if (power_down) {
        wifi_prov_shutdown();
    }

    vTaskDelete(NULL);
}

void tcp_server_start(void)
{
    if (s_running) {
        return;
    }
    s_running = true;
    if (xTaskCreate(tcp_server_task, "monocle_tcp", 4096, NULL, 5, &s_task) !=
        pdPASS) {
        ESP_LOGE(TAG, "could not start the server task");
        s_running = false;
    }
}

void tcp_server_stop(void)
{
    s_running = false;   /* the task notices within SOCKET_TIMEOUT_MS */
}
