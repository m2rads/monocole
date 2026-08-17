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
 * Generous on purpose: 800 ms was shorter than an ordinary mid-sentence pause,
 * so thinking for a moment ended the recording and the second half of the
 * question was lost.
 *
 * The cost is directly felt — this is dead air between finishing a sentence
 * and the reply starting, because nothing downstream can begin until the
 * utterance closes. 1500-2000 ms is the usual sweet spot if this feels
 * sluggish; it is one number.
 */
#define VOICE_END_SILENCE_MS    5000

/*
 * How long to wait for the speaker to begin, after the wake word.
 *
 * Nobody starts talking the instant the word leaves their mouth: they pause,
 * often to check the panel says "listening". Without this the silence timer
 * starts immediately and a normal pause ends the utterance before a word is
 * said — which shows up as an 800 ms capture containing nothing, and a
 * transcript of "[BLANK_AUDIO]".
 *
 * Generous, because the cost of waiting is a second of nothing while the cost
 * of being too strict is losing the sentence entirely.
 */
#define VOICE_LEAD_IN_MS        5000

/*
 * A hard ceiling on one utterance.
 *
 * The VAD is the normal way out. This exists because a noisy room can keep
 * VAD_SPEECH asserted indefinitely, and an utterance that never ends is one
 * that never gets transcribed — a stuck session is worse than a truncated one.
 *
 * 30 s because that is whisper's window: it processes audio in 30-second
 * chunks, so an utterance longer than this gains nothing in one pass anyway.
 */
#define VOICE_MAX_MS            30000

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
 * Set to 0 to leave every model alone — which is where it sits now. 0.50 was
 * tried on hardware and made the device wake on ordinary conversation, which
 * is far worse than having to repeat yourself: every false trigger opens a
 * session, and once the app turns sessions into history it also files a
 * transcript of whatever was being said at the time. The shipped defaults are
 * the better trade, with Hi ESP as the word that reliably works.
 */
#define VOICE_TTS_THRESHOLD     0.0f

static esp_afe_sr_data_t *s_afe_data;
static const esp_afe_sr_iface_t *s_afe;
static volatile bool s_listening;

/* Set once at init: how many samples one feed() wants. */
static int s_feed_samples;

/* Streaming state, touched only by the feed task — see voice_stream(). */
static adpcm_state_t s_adpcm;
static int16_t s_pcm[ADPCM_FRAME_SAMPLES];
static int s_pcm_used;
static uint16_t s_seq;

/* Raised by the fetch task when an utterance begins, cleared by the feed task
 * once it has restarted its encoder. A flag rather than a direct reset because
 * the two run on different tasks and a half-written frame is not worth a
 * mutex on the audio path. */
static volatile bool s_stream_restart;

/* One-pole DC blocker state, for the raw microphone path. */
static int32_t s_dc_prev_in;
static int32_t s_dc_prev_out;

static void voice_stream(const int16_t *samples, int count);

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

        /* The same samples go on the wire, unprocessed. Streaming from here
         * rather than from the fetch task is what keeps AFE's noise
         * suppression out of the audio whisper sees. */
        if (s_listening) {
            voice_stream(buffer, (int)got);
        }
    }
}

/*
 * Removes the microphone's DC offset.
 *
 * The PDM mic sits on an offset of roughly 1400 counts. Left in, ADPCM spends
 * its dynamic range tracking a constant instead of the speech on top of it,
 * and the predictor takes the start of every utterance to climb there.
 *
 * A one-pole high pass: y[n] = x[n] - x[n-1] + 0.995 * y[n-1]. The 0.995 is
 * 4079/4096 so the multiply stays integer.
 */
static int16_t
dc_block(int16_t sample)
{
    int32_t out = sample - s_dc_prev_in + ((s_dc_prev_out * 4079) >> 12);
    s_dc_prev_in = sample;
    s_dc_prev_out = out;

    if (out > 32767) {
        return 32767;
    }
    if (out < -32768) {
        return -32768;
    }
    return (int16_t)out;
}

/*
 * Encodes microphone audio and puts it on the air.
 *
 * **Deliberately the raw microphone, not AFE's output.** The front end applies
 * noise suppression and gain tuned for waking on a keyword, and suppression
 * works by attenuating noise-like content — which is exactly what consonants
 * are. Measured on real utterances, everything above 3 kHz was all but gone,
 * so vowels survived and `s`, `f`, `t` and `sh` did not. Whisper does its own
 * noise handling and would rather have the unprocessed signal. AFE still runs;
 * it just decides *when* to listen rather than *what* gets sent.
 *
 * Frames are a fixed 512 samples on the wire, but a mic read need not hand
 * back exactly that many, so samples accumulate here and go out a full frame
 * at a time. The leftovers of one read start the next frame.
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

    if (s_stream_restart) {
        /* A new utterance: restart the numbering and the codec so the app can
         * tell one from the next without being told where the boundary is. */
        s_stream_restart = false;
        s_seq = 0;
        s_pcm_used = 0;
        adpcm_reset(&s_adpcm);
    }

    for (int i = 0; i < count; i++) {
        s_pcm[s_pcm_used++] = dc_block(samples[i]);
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

    /* The feed task owns the encoder, so ask it to restart rather than
     * reaching into its state from here. */
    s_stream_restart = true;

    /* wakenet_model_index says *which wake word* fired, which is the number
     * that matters when one of them is triggering on ordinary speech;
     * wake_word_index only distinguishes words within a single model. */
    ESP_LOGI(TAG, "wake word detected (model %d, word %d, %.1f dB)",
             result->wakenet_model_index, result->wake_word_index,
             result->data_volume);

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
    /* The silence countdown does not start until the speaker actually
     * begins — see VOICE_LEAD_IN_MS. */
    bool heard_speech = false;

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
                heard_speech = false;
            }
            continue;
        }

        elapsed_ms += frame_ms;

        if (result->vad_state == VAD_SPEECH) {
            heard_speech = true;
            silence_ms = 0;
        } else {
            silence_ms += frame_ms;
        }

        if (heard_speech) {
            /* They spoke and have now stopped for long enough. */
            if (silence_ms >= VOICE_END_SILENCE_MS) {
                voice_end_utterance(MONOCLE_VOICE_END_VAD);
            } else if (elapsed_ms >= VOICE_MAX_MS) {
                voice_end_utterance(MONOCLE_VOICE_END_CAPPED);
            }
        } else if (silence_ms >= VOICE_LEAD_IN_MS) {
            /* Woken, but nobody ever started talking — a false trigger, or
             * the wake word said on its own. Ending on the same VAD reason
             * is right: there is genuinely no speech to transcribe. */
            ESP_LOGI(TAG, "no speech after the wake word");
            voice_end_utterance(MONOCLE_VOICE_END_VAD);
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
