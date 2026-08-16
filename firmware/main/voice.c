#include "voice.h"

#include <string.h>

#include "esp_afe_config.h"
#include "esp_afe_sr_iface.h"
#include "esp_afe_sr_models.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "esp_wn_models.h"
#include "model_path.h"

#include "adpcm.h"
#include "bleprph.h"
#include "display.h"
#include "mic.h"

static const char *TAG = "monocle_voice";

/*
 * How long the speaker has to stop before the utterance is considered over.
 *
 * Long enough to survive the pause in the middle of a sentence, short enough
 * that the reply does not feel delayed. AFE's own VAD reports per frame; this
 * is the run of silent frames we require on top of it.
 */
#define VOICE_END_SILENCE_MS    800

/*
 * A hard ceiling on one utterance.
 *
 * The VAD is the normal way out. This exists because a noisy room can keep
 * VAD_SPEECH asserted indefinitely, and an utterance that never ends is one
 * that never gets transcribed — a stuck session is worse than a truncated one.
 */
#define VOICE_MAX_MS            15000

/* AFE's tasks are the ones doing real work; these two only shuttle buffers. */
#define VOICE_TASK_STACK        4096
#define VOICE_TASK_PRIO         5

/*
 * How certain WakeNet has to be before it says the wake word was spoken, for
 * models trained on synthesized speech.
 *
 * Valid range is 0.4 to 0.9999; models ship with their own default, around
 * 0.63. Lower is more sensitive and more false triggers — and false triggers
 * are expensive here, because each one opens a session and puts "listening"
 * in front of the wearer.
 *
 * Applied only to `_tts` models. Espressif trains those on synthesized speech
 * rather than real recordings, and they are correspondingly less sure about
 * actual voices; of the English models only "Hi ESP" and "Alexa" use real
 * data. Those keep their shipped default, because they do not need the help
 * and lowering them would only buy false triggers.
 *
 * Set to 0 to leave every model alone.
 */
#define VOICE_TTS_THRESHOLD     0.5f

static esp_afe_sr_data_t *s_afe_data;
static const esp_afe_sr_iface_t *s_afe;
static volatile bool s_listening;

/* Set once at init: how many samples one feed() wants. */
static int s_feed_samples;

/* Streaming state, touched only by the fetch task. */
static adpcm_state_t s_adpcm;
static int16_t s_pcm[ADPCM_FRAME_SAMPLES];
static int s_pcm_used;
static uint16_t s_seq;

bool
voice_is_listening(void)
{
    return s_listening;
}

/*
 * Pulls samples off the mic and hands them to the front end.
 *
 * Nothing clever belongs here. AFE runs its own processing task internally,
 * so this one only has to keep up — if it falls behind, the ring buffer fills
 * and fetch() starts reporting a busy pipeline.
 */
static void
voice_feed_task(void *arg)
{
    int16_t *buffer = heap_caps_malloc(s_feed_samples * sizeof(int16_t),
                                       MALLOC_CAP_INTERNAL);
    if (buffer == NULL) {
        ESP_LOGE(TAG, "no memory for the feed buffer");
        vTaskDelete(NULL);
        return;
    }

    while (true) {
        size_t got = mic_read(buffer, s_feed_samples, 100);
        if (got < (size_t)s_feed_samples) {
            /* A short read means the mic gave us less than a full chunk;
             * padding with silence keeps the front end's timing honest rather
             * than feeding it a partial frame. */
            memset(buffer + got, 0, (s_feed_samples - got) * sizeof(int16_t));
        }
        s_afe->feed(s_afe_data, buffer);
    }
}

/*
 * Encodes what the front end produced and puts it on the air.
 *
 * Frames are a fixed 512 samples on the wire, but nothing guarantees a fetch
 * hands back exactly that many, so samples accumulate here and go out a full
 * frame at a time. The leftovers of one fetch start the next frame.
 *
 * Encoder state runs continuously across the utterance; each frame's header
 * records where it started, taken *before* encoding. That is what makes a
 * frame decodable alone, and why a dropped notification costs only its own
 * 32 ms instead of the rest of the sentence.
 */
static void
voice_stream(const int16_t *samples, int count)
{
    if (!gatt_svr_voice_is_subscribed()) {
        /* Nobody is listening, so encoding would be wasted CPU on a device
         * that is already running a neural net continuously. */
        return;
    }

    for (int i = 0; i < count; i++) {
        s_pcm[s_pcm_used++] = samples[i];
        if (s_pcm_used < ADPCM_FRAME_SAMPLES) {
            continue;
        }

        int16_t predictor = s_adpcm.predictor;
        uint8_t step_index = s_adpcm.step_index;
        uint8_t payload[ADPCM_FRAME_BYTES];

        adpcm_encode(&s_adpcm, s_pcm, ADPCM_FRAME_SAMPLES, payload);
        gatt_svr_notify_voice(s_seq++, predictor, step_index,
                              payload, sizeof payload);
        s_pcm_used = 0;
    }
}

static void
voice_start_utterance(const afe_fetch_result_t *result)
{
    s_listening = true;

    /* A fresh utterance starts the numbering and the codec over, so the app
     * can tell one from the next without being told where the boundary is. */
    s_seq = 0;
    s_pcm_used = 0;
    adpcm_reset(&s_adpcm);

    ESP_LOGI(TAG, "wake word detected (index %d, %.1f dB)",
             result->wake_word_index, result->data_volume);

    /* The wearer needs to know it heard them before the reply exists —
     * inference takes seconds and a blank panel is indistinguishable from a
     * device that ignored you. */
    display_show("listening");
    gatt_svr_notify_status(MONOCLE_STATUS_VOICE_STARTED, NULL, 0);
}

static void
voice_end_utterance(uint8_t reason)
{
    static const char *reasons[] = { "silence", "hit the length cap", "error" };

    s_listening = false;
    ESP_LOGI(TAG, "utterance ended (%s)",
             reason < 3 ? reasons[reason] : "unknown");

    gatt_svr_notify_status(MONOCLE_STATUS_VOICE_ENDED, &reason, 1);

    /* Back to whatever the panel rests on, rather than blank. A screen going
     * dark half a second after you speak reads as the device ignoring you,
     * and this is the only output the wearer has. Once the app handles voice
     * it will overwrite this with a thinking indicator and then the reply. */
    display_show_idle(gatt_svr_is_connected());
}

/*
 * Reads processed audio out of the front end and runs the session state
 * machine over it.
 *
 * Two states: waiting for the wake word, and listening to what follows.
 * Detections are ignored while listening, so saying the wake word mid-sentence
 * does not restart the utterance.
 */
static void
voice_fetch_task(void *arg)
{
    int silence_ms = 0;
    int elapsed_ms = 0;

    while (true) {
        afe_fetch_result_t *result = s_afe->fetch(s_afe_data);
        if (result == NULL || result->ret_value == ESP_FAIL) {
            if (s_listening) {
                voice_end_utterance(MONOCLE_VOICE_END_ERROR);
            }
            ESP_LOGW(TAG, "fetch failed");
            continue;
        }

        /* One fetch is one frame; deriving its length from the data rather
         * than assuming 32 ms keeps the timers right if the front end is ever
         * reconfigured. */
        int frame_ms = (result->data_size / (int)sizeof(int16_t)) * 1000
                       / MIC_SAMPLE_RATE_HZ;

        if (!s_listening) {
            if (result->wakeup_state == WAKENET_DETECTED) {
                voice_start_utterance(result);
                silence_ms = 0;
                elapsed_ms = 0;
            }
            continue;
        }

        voice_stream(result->data,
                     result->data_size / (int)sizeof(int16_t));

        elapsed_ms += frame_ms;
        silence_ms = (result->vad_state == VAD_SPEECH) ? 0
                                                       : silence_ms + frame_ms;

        if (silence_ms >= VOICE_END_SILENCE_MS) {
            voice_end_utterance(MONOCLE_VOICE_END_VAD);
        } else if (elapsed_ms >= VOICE_MAX_MS) {
            voice_end_utterance(MONOCLE_VOICE_END_CAPPED);
        }
    }
}

/*
 * Lists the loaded wake words and gives the synthesized-speech ones a hand.
 *
 * WakeNet numbers its models from 1 in the order they were loaded, which is
 * the order they appear in the model list. If that assumption were ever wrong
 * the cost is a threshold applied to the wrong word — a sensitivity change,
 * not a failure — and the log below makes it visible.
 */
static void
voice_tune_thresholds(srmodel_list_t *models)
{
    int index = 0;

    for (int i = 0; i < models->num; i++) {
        const char *name = models->model_name[i];
        if (strncmp(name, ESP_WN_PREFIX, strlen(ESP_WN_PREFIX)) != 0) {
            continue;   /* not a wake word model */
        }

        index++;
        bool synthesized = strstr(name, "_tts") != NULL;
        ESP_LOGI(TAG, "wake word %d: %s%s", index, name,
                 synthesized ? " (trained on synthesized speech)" : "");

        /* The API only addresses the first two. */
        if (!synthesized || VOICE_TTS_THRESHOLD <= 0.0f || index > 2) {
            continue;
        }

        if (s_afe->set_wakenet_threshold(s_afe_data, index,
                                         VOICE_TTS_THRESHOLD) != 1) {
            ESP_LOGW(TAG, "could not lower the threshold for %s", name);
        } else {
            ESP_LOGI(TAG, "  threshold lowered to %.2f", VOICE_TTS_THRESHOLD);
        }
    }
}

esp_err_t
voice_init(void)
{
    srmodel_list_t *models = esp_srmodel_init("model");
    if (models == NULL || models->num == 0) {
        ESP_LOGE(TAG, "no speech models in the 'model' partition");
        return ESP_ERR_NOT_FOUND;
    }

    /* "M" is one microphone and no reference channel: the board has a single
     * PDM mic, and echo cancellation would want a copy of what we are playing,
     * which on a device with no speaker is nothing. */
    afe_config_t *config = afe_config_init("M", models, AFE_TYPE_SR,
                                           AFE_MODE_LOW_COST);
    if (config == NULL) {
        ESP_LOGE(TAG, "afe_config_init failed");
        return ESP_FAIL;
    }

    config->wakenet_init = true;
    config->vad_init = true;

    /* The front end reports VAD per frame; the run-length decision is ours, in
     * the fetch task, so keep its own minimum short enough not to fight it. */
    config->vad_min_speech_ms = 128;
    config->vad_min_noise_ms = 256;

    s_afe = esp_afe_handle_from_config(config);
    s_afe_data = s_afe->create_from_config(config);
    afe_config_free(config);

    if (s_afe_data == NULL) {
        ESP_LOGE(TAG, "could not create the audio front end");
        return ESP_FAIL;
    }

    s_feed_samples = s_afe->get_feed_chunksize(s_afe_data)
                     * s_afe->get_feed_channel_num(s_afe_data);

    voice_tune_thresholds(models);

    ESP_LOGI(TAG, "listening for the wake word (%d samples per feed, "
                  "%d ms of silence ends an utterance)",
             s_feed_samples, VOICE_END_SILENCE_MS);

    xTaskCreate(voice_feed_task, "voice_feed", VOICE_TASK_STACK, NULL,
                VOICE_TASK_PRIO, NULL);
    xTaskCreate(voice_fetch_task, "voice_fetch", VOICE_TASK_STACK, NULL,
                VOICE_TASK_PRIO, NULL);

    return ESP_OK;
}
