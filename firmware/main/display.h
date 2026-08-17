/*
 * The monocle's panel.
 *
 * Today a 128x64 SSD1306 OLED on I2C — a stand-in for the micro-LED the
 * product will use. Nothing outside this module should assume those
 * dimensions; ask for DISPLAY_COLS/DISPLAY_ROWS instead, so swapping the panel
 * stays a change in one file.
 */

#pragma once

#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

#define DISPLAY_WIDTH       128
#define DISPLAY_HEIGHT      64

/* A 5x7 glyph in a 6x8 cell: 21 characters across, 8 lines down. That is ~168
 * characters on screen — far less than a typical model reply, which is why
 * what to show is a product decision (see Future work in docs/protocol.md). */
#define DISPLAY_CELL_W      6
#define DISPLAY_CELL_H      8
#define DISPLAY_COLS        (DISPLAY_WIDTH / DISPLAY_CELL_W)
#define DISPLAY_ROWS        (DISPLAY_HEIGHT / DISPLAY_CELL_H)

/* Ops carried by the display characteristic — see docs/protocol.md. */
enum display_op {
    DISPLAY_OP_CLEAR  = 0,
    DISPLAY_OP_SET    = 1,   /* replace the screen with this text */
    DISPLAY_OP_APPEND = 2,   /* add to what is there; how tokens will stream */
};

/* Longest text one write can carry — this buffer, not the link. An ATT write
 * holds MTU-3 bytes of value, which at the negotiated MTU of 512 is 509. This
 * limit dates from NimBLE's old default MTU of 256 and stayed put when the MTU
 * was raised, so it leaves headroom. Mirrored by DISPLAY_TEXT_MAX in
 * src-tauri/src/ble.rs. */
#define DISPLAY_TEXT_MAX    252

/* Brings up I2C and the panel, and starts the render task. Call once, at
 * boot, before anything posts to it. */
esp_err_t display_init(void);

/* Queues a screen update. Safe to call from any task — including the NimBLE
 * host task, which must never block on I2C. Rendering happens later on the
 * display task; a full update takes ~25 ms.
 *
 * Text longer than the panel is paginated rather than cut off: the first
 * screenful goes up, and the rest follows a page at a time, each held long
 * enough to read. `set` and `clear` return the reader to the top; `append`
 * leaves their place alone, so generation can run ahead of reading.
 *
 * Returns false if the queue is full, which drops the update: the panel is
 * advisory, and blocking a caller to guarantee a frame is the wrong trade. */
bool display_post(uint8_t op, const char *text, size_t len);

/* Convenience wrapper for a NUL-terminated replacement. */
void display_show(const char *text);

/* Shows the panel's resting state: what it displays when nothing else is
 * happening. Defined here so boot, disconnect and the end of a voice session
 * all say the same words instead of each inventing their own.
 *
 * `app_connected` picks between waiting for the app and being ready for it. A
 * placeholder until the panel grows a real UI. */
void display_show_idle(bool app_connected);

/* Blanks the panel. */
void display_clear(void);

#ifdef __cplusplus
}
#endif
