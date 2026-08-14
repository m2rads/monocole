#include "mic.h"

#include <math.h>
#include <stdio.h>
#include <string.h>

#include "driver/i2s_pdm.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

static const char *TAG = "monocle_mic";

/*
 * Wiring on the XIAO ESP32-S3 Sense. These are fixed on the expansion board,
 * not a choice — the mic is soldered to them. No conflict with the panel,
 * which is on GPIO5/6.
 */
#define MIC_CLK_GPIO    GPIO_NUM_42
#define MIC_DIN_GPIO    GPIO_NUM_41

static i2s_chan_handle_t s_rx;

esp_err_t
mic_init(void)
{
    esp_err_t err;

    if (s_rx != NULL) {
        return ESP_OK;
    }

    i2s_chan_config_t chan_cfg =
        I2S_CHANNEL_DEFAULT_CONFIG(I2S_NUM_AUTO, I2S_ROLE_MASTER);
    err = i2s_new_channel(&chan_cfg, NULL, &s_rx);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "could not allocate an i2s channel; err=%d", err);
        return err;
    }

    /* The default slot config resolves to the PCM variant on this chip, which
     * means the hardware filter converts PDM for us and reads come back as
     * plain signed 16-bit samples. On a part without that filter the same
     * macro would hand back a raw bitstream, so this is worth knowing rather
     * than assuming. */
    i2s_pdm_rx_config_t pdm_cfg = {
        .clk_cfg = I2S_PDM_RX_CLK_DEFAULT_CONFIG(MIC_SAMPLE_RATE_HZ),
        .slot_cfg = I2S_PDM_RX_SLOT_DEFAULT_CONFIG(I2S_DATA_BIT_WIDTH_16BIT,
                                                   I2S_SLOT_MODE_MONO),
        .gpio_cfg = {
            .clk = MIC_CLK_GPIO,
            .din = MIC_DIN_GPIO,
            .invert_flags = {
                .clk_inv = false,
            },
        },
    };

    err = i2s_channel_init_pdm_rx_mode(s_rx, &pdm_cfg);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "could not configure pdm rx; err=%d", err);
        goto fail;
    }

    err = i2s_channel_enable(s_rx);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "could not enable the i2s channel; err=%d", err);
        goto fail;
    }

    ESP_LOGI(TAG, "mic ready (%d Hz mono, clk GPIO%d, din GPIO%d)",
             MIC_SAMPLE_RATE_HZ, MIC_CLK_GPIO, MIC_DIN_GPIO);
    return ESP_OK;

fail:
    i2s_del_channel(s_rx);
    s_rx = NULL;
    return err;
}

size_t
mic_read(int16_t *out, size_t samples, uint32_t timeout_ms)
{
    size_t read = 0;

    if (s_rx == NULL || out == NULL || samples == 0) {
        return 0;
    }

    esp_err_t err = i2s_channel_read(s_rx, out, samples * sizeof(int16_t),
                                     &read, pdMS_TO_TICKS(timeout_ms));
    if (err != ESP_OK && err != ESP_ERR_TIMEOUT) {
        ESP_LOGW(TAG, "i2s read failed; err=%d", err);
        return 0;
    }

    return read / sizeof(int16_t);
}

/* ---- Temporary capture dump. See the note in mic.h. ---- */

static const char BASE64[] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/*
 * Base64 rather than hex: it is a third smaller, and at 115200 baud the
 * difference is several seconds of staring at a terminal.
 */
static void
print_base64(const uint8_t *data, size_t len)
{
    char line[77];
    size_t col = 0;

    for (size_t i = 0; i < len; i += 3) {
        uint32_t block = (uint32_t)data[i] << 16;
        size_t remaining = len - i;
        if (remaining > 1) {
            block |= (uint32_t)data[i + 1] << 8;
        }
        if (remaining > 2) {
            block |= data[i + 2];
        }

        line[col++] = BASE64[(block >> 18) & 0x3f];
        line[col++] = BASE64[(block >> 12) & 0x3f];
        line[col++] = remaining > 1 ? BASE64[(block >> 6) & 0x3f] : '=';
        line[col++] = remaining > 2 ? BASE64[block & 0x3f] : '=';

        if (col >= 76) {
            line[col] = '\0';
            printf("%s\n", line);
            /* The console is a 115200-baud UART with a finite buffer; a tight
             * loop of 100 KB outruns it and the tail arrives as garbage. */
            vTaskDelay(pdMS_TO_TICKS(2));
            col = 0;
        }
    }

    if (col > 0) {
        line[col] = '\0';
        printf("%s\n", line);
    }
}

void
mic_dump_to_console(float seconds)
{
    size_t samples = (size_t)(seconds * MIC_SAMPLE_RATE_HZ);
    size_t bytes = samples * sizeof(int16_t);

    /* PSRAM: ~96 KB for three seconds would be a serious dent in the ~124 KB
     * of internal heap left once BLE, Wi-Fi and the panel have taken theirs. */
    int16_t *pcm = heap_caps_malloc(bytes, MALLOC_CAP_SPIRAM);
    if (pcm == NULL) {
        ESP_LOGE(TAG, "could not allocate %u bytes for the capture", (unsigned)bytes);
        return;
    }

    ESP_LOGI(TAG, "capturing %.1f s — say something now", seconds);

    size_t got = 0;
    while (got < samples) {
        size_t chunk = mic_read(pcm + got, samples - got, 1000);
        if (chunk == 0) {
            ESP_LOGE(TAG, "capture stalled after %u samples", (unsigned)got);
            break;
        }
        got += chunk;
    }

    /* A quick verdict before the transfer, so a dead mic is obvious without
     * decoding anything: silence reads as a flat zero or a stuck value. */
    int32_t min = 32767, max = -32768;
    int64_t sum_squares = 0;
    for (size_t i = 0; i < got; i++) {
        if (pcm[i] < min) min = pcm[i];
        if (pcm[i] > max) max = pcm[i];
        sum_squares += (int64_t)pcm[i] * pcm[i];
    }
    ESP_LOGI(TAG, "captured %u samples: min=%d max=%d rms=%d",
             (unsigned)got, (int)min, (int)max,
             got ? (int)sqrt((double)(sum_squares / (int64_t)got)) : 0);

    printf("---BEGIN MONOCLE PCM %u %u---\n",
           (unsigned)MIC_SAMPLE_RATE_HZ, (unsigned)got);
    print_base64((const uint8_t *)pcm, got * sizeof(int16_t));
    printf("---END MONOCLE PCM---\n");

    heap_caps_free(pcm);
}
