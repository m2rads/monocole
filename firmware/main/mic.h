/*
 * The monocle's microphone.
 *
 * A PDM mic on the XIAO ESP32-S3 Sense expansion board, read over I2S with the
 * chip's PDM-to-PCM filter doing the conversion, so what comes out is ordinary
 * 16-bit PCM rather than a bitstream to decimate in software.
 *
 * 16 kHz mono, because that is what ESP-SR's front end takes and what IMA
 * ADPCM is sized against in docs/protocol.md. Nothing here knows about wake
 * words or encoding — this module's whole job is to produce samples.
 */

#pragma once

#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

#define MIC_SAMPLE_RATE_HZ   16000

/* 32 ms of audio: one AFE feed chunk and one voice frame on the wire. Reading
 * in this unit means nothing downstream has to regroup samples. */
#define MIC_FRAME_SAMPLES    512

/* Brings up the I2S channel. Call once, at boot. */
esp_err_t mic_init(void);

/* Reads up to `samples` 16-bit samples, blocking for at most `timeout_ms`.
 * Returns how many were actually read — short reads are normal at a timeout
 * and are not an error. */
size_t mic_read(int16_t *out, size_t samples, uint32_t timeout_ms);

/* Records `seconds` of audio and prints it to the console as base64, for
 * firmware/test/capture_mic.py to turn back into a WAV.
 *
 * TODO(voice-spike): this is the milestone-3 gate — prove the capture path
 * produces intelligible audio before a wake word detector is wired to it, so
 * that a silent pipeline never has two possible causes. Delete once voice
 * streams over BLE for real. Enable with CONFIG_MONOCLE_MIC_DUMP_AT_BOOT.
 */
void mic_dump_to_console(float seconds);

#ifdef __cplusplus
}
#endif
