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

/* Longest text one write can carry: a single ATT write at the MTU macOS
 * negotiates (256), minus the op byte. */
#define DISPLAY_TEXT_MAX    253

/* Brings up I2C and the panel, and starts the render task. Call once, at
 * boot, before anything posts to it. */
esp_err_t display_init(void);

/* Queues a screen update. Safe to call from any task — including the NimBLE
 * host task, which must never block on I2C. Rendering happens later on the
 * display task; a full update takes ~25 ms.
 *
 * Returns false if the queue is full, which drops the update: the panel is
 * advisory, and blocking a caller to guarantee a frame is the wrong trade. */
bool display_post(uint8_t op, const char *text, size_t len);

/* Convenience wrapper for a NUL-terminated replacement. */
void display_show(const char *text);

/* Blanks the panel. */
void display_clear(void);

#ifdef __cplusplus
}
#endif
