/*
 * Wake word detection and voice session state.
 *
 * Owns ESP-SR's audio front end: the mic feeds it, WakeNet listens for "Hi
 * ESP", and its VAD decides when the speaker has finished. What crosses the
 * wire is only the *event* — voice_started, voice_ended — never the trigger
 * itself, so the wake word can change without the app knowing. See
 * docs/protocol.md, "What starts a voice session".
 *
 * Encoding and streaming the audio is the next milestone; the seam it plugs
 * into is marked in voice.c.
 */

#pragma once

#include <stdbool.h>

#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Creates the front end and starts the feed and fetch tasks. Requires
 * mic_init() to have succeeded. Safe to call once, at boot. */
esp_err_t voice_init(void);

/* Whether an utterance is in progress right now. */
bool voice_is_listening(void);

#ifdef __cplusplus
}
#endif
