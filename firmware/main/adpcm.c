#include "adpcm.h"

/*
 * The standard IMA tables. The step index walks the step table; the index
 * table says how far each nibble moves it, which is how the codec adapts to
 * loud and quiet passages.
 */
static const int8_t INDEX_TABLE[16] = {
    -1, -1, -1, -1, 2, 4, 6, 8,
    -1, -1, -1, -1, 2, 4, 6, 8,
};

static const int16_t STEP_TABLE[89] = {
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37,
    41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173,
    190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658,
    724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289,
    16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
};

#define MAX_STEP_INDEX  ((int)(sizeof STEP_TABLE / sizeof STEP_TABLE[0]) - 1)

static int16_t
clamp_sample(int value)
{
    if (value > 32767) {
        return 32767;
    }
    if (value < -32768) {
        return -32768;
    }
    return (int16_t)value;
}

/*
 * Applies one code to the state, exactly as a decoder would.
 *
 * The encoder has to track the decoder's reconstruction rather than the true
 * input, or the two drift apart within a few dozen samples and the audio
 * degrades into noise.
 */
static void
apply_nibble(adpcm_state_t *state, uint8_t nibble)
{
    int step = STEP_TABLE[state->step_index];
    int diff = step >> 3;

    if (nibble & 4) {
        diff += step;
    }
    if (nibble & 2) {
        diff += step >> 1;
    }
    if (nibble & 1) {
        diff += step >> 2;
    }
    if (nibble & 8) {
        diff = -diff;
    }

    state->predictor = clamp_sample(state->predictor + diff);

    int index = state->step_index + INDEX_TABLE[nibble];
    if (index < 0) {
        index = 0;
    } else if (index > MAX_STEP_INDEX) {
        index = MAX_STEP_INDEX;
    }
    state->step_index = (uint8_t)index;
}

static uint8_t
encode_sample(adpcm_state_t *state, int16_t sample)
{
    int step = STEP_TABLE[state->step_index];
    int diff = sample - state->predictor;
    uint8_t nibble = 0;

    if (diff < 0) {
        nibble = 8;
        diff = -diff;
    }

    if (diff >= step) {
        nibble |= 4;
        diff -= step;
    }
    step >>= 1;
    if (diff >= step) {
        nibble |= 2;
        diff -= step;
    }
    step >>= 1;
    if (diff >= step) {
        nibble |= 1;
    }

    apply_nibble(state, nibble);
    return nibble;
}

void
adpcm_reset(adpcm_state_t *state)
{
    state->predictor = 0;
    state->step_index = 0;
}

void
adpcm_encode(adpcm_state_t *state, const int16_t *samples, size_t count,
             uint8_t *out)
{
    for (size_t i = 0; i + 1 < count; i += 2) {
        /* Low nibble first: what IMA specifies, and what any other decoder
         * will assume when someone opens the resulting WAV. */
        uint8_t low = encode_sample(state, samples[i]);
        uint8_t high = encode_sample(state, samples[i + 1]);
        out[i / 2] = (uint8_t)((high << 4) | low);
    }
}
