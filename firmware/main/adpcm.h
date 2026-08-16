/*
 * IMA ADPCM encoding for the voice characteristic.
 *
 * 4 bits per sample, so 16 kHz mono comes out at ~64 kbps — inside BLE's
 * real-world budget on a radio that is already up for control traffic.
 *
 * The encoder runs continuously across an utterance and never resets between
 * frames. Each frame's header instead *snapshots* the state it began from,
 * which is what lets the app decode any frame without the ones before it.
 * Resetting per frame would also be decodable and sounds worse: the predictor
 * would climb from zero at the top of all 31 frames a second, which is an
 * audible buzz. See docs/protocol.md.
 *
 * firmware/test/adpcm.py is an independent implementation of the same format,
 * deliberately not shared with this one, so a drift on either side fails a
 * test rather than being mirrored into it.
 */

#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Samples in one frame: 512, which is 32 ms at 16 kHz — one AFE chunk, and
 * one BLE notification. */
#define ADPCM_FRAME_SAMPLES  512

/* What those samples compress to, at two per byte. */
#define ADPCM_FRAME_BYTES    (ADPCM_FRAME_SAMPLES / 2)

/* The running decoder state. Carried across frames, snapshotted into each. */
typedef struct {
    int16_t predictor;
    uint8_t step_index;
} adpcm_state_t;

/* Returns the encoder to the start of an utterance. */
void adpcm_reset(adpcm_state_t *state);

/* Encodes `count` samples (must be even) into `count / 2` bytes, advancing
 * `state`. Snapshot the state *before* calling this if the result is going
 * into a frame header. */
void adpcm_encode(adpcm_state_t *state, const int16_t *samples, size_t count,
                  uint8_t *out);

#ifdef __cplusplus
}
#endif
