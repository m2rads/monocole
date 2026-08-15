/*
 * SSD1306 OLED panel driver glue plus a minimal text renderer.
 *
 * The panel driver itself ships with ESP-IDF (esp_lcd_panel_ssd1306); fonts do
 * not, so a 5x7 ASCII table lives here. Rendering is done into a local
 * framebuffer and pushed in one transfer, because a full update is ~25 ms on
 * a 400 kHz bus and doing it per character would be visibly slow.
 */

#include <string.h>

#include "driver/i2c_master.h"
#include "esp_lcd_io_i2c.h"
#include "esp_lcd_panel_dev.h"
#include "esp_lcd_panel_ops.h"
#include "esp_lcd_panel_ssd1306.h"
#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/task.h"

#include "display.h"

static const char *TAG = "monocle_display";

/* Wiring on the XIAO ESP32-S3: SDA to D4, SCL to D5. */
#define DISPLAY_SDA_GPIO        5
#define DISPLAY_SCL_GPIO        6
#define DISPLAY_I2C_HZ          400000

/* Most 0.96" modules answer at 0x3C; a few strap the address pin to 0x3D. Both
 * are probed at init so a differently strapped board is a log line rather than
 * a silent blank screen. */
#define DISPLAY_ADDR_PRIMARY    0x3C
#define DISPLAY_ADDR_ALTERNATE  0x3D
#define DISPLAY_PROBE_TIMEOUT_MS 100

/* The SSD1306 lays its memory out in pages of 8 vertical pixels, so one byte
 * spans y..y+7 at a single x. Bit 0 is the topmost pixel of the page. */
#define DISPLAY_PAGES           (DISPLAY_HEIGHT / 8)
#define DISPLAY_FB_LEN          (DISPLAY_WIDTH * DISPLAY_PAGES)

/* Backing store for the whole response, not just what fits on screen: a reply
 * runs 500-2000 characters and the reader is walked through it a page at a
 * time. Overflow drops from the front, which is only reachable by an answer
 * longer than this. */
#define DISPLAY_TEXT_BUFFER     4096

/*
 * How long a full page stays up before the next one replaces it.
 *
 * Proportional to how much is actually on the page — two lines do not need the
 * dwell of eight. Roughly 250 words per minute is 20 ms per character; the
 * doubled figure leaves room to look away and back, which is the normal way
 * someone uses a monocle.
 *
 * There is no way to go back. Erring long is the safer mistake.
 */
#define PAGE_HOLD_BASE_MS       2000
#define PAGE_HOLD_PER_CHAR_MS   40
#define PAGE_HOLD_MAX_MS        8000

/* Shallow, and deliberately so: if updates are arriving faster than the panel
 * can draw them, the newest matters and the backlog does not. */
#define DISPLAY_QUEUE_DEPTH     4

struct display_msg {
    uint8_t op;
    uint16_t len;
    char text[DISPLAY_TEXT_MAX];
};

static esp_lcd_panel_handle_t s_panel;
static uint8_t s_framebuffer[DISPLAY_FB_LEN];
static QueueHandle_t s_queue;

/*
 * 5x7 font, ASCII 0x20 to 0x7E. Each glyph is five column bytes; bit 0 is the
 * top row. The sixth column of the cell is left blank as inter-character
 * spacing, which is why nothing here is six bytes wide.
 */
#define FONT_FIRST_CHAR         0x20
#define FONT_LAST_CHAR          0x7E
#define FONT_GLYPH_W            5

static const uint8_t FONT_5X7[][FONT_GLYPH_W] = {
    {0x00, 0x00, 0x00, 0x00, 0x00}, /* space */
    {0x00, 0x00, 0x5F, 0x00, 0x00}, /* ! */
    {0x00, 0x07, 0x00, 0x07, 0x00}, /* " */
    {0x14, 0x7F, 0x14, 0x7F, 0x14}, /* # */
    {0x24, 0x2A, 0x7F, 0x2A, 0x12}, /* $ */
    {0x23, 0x13, 0x08, 0x64, 0x62}, /* % */
    {0x36, 0x49, 0x55, 0x22, 0x50}, /* & */
    {0x00, 0x05, 0x03, 0x00, 0x00}, /* ' */
    {0x00, 0x1C, 0x22, 0x41, 0x00}, /* ( */
    {0x00, 0x41, 0x22, 0x1C, 0x00}, /* ) */
    {0x14, 0x08, 0x3E, 0x08, 0x14}, /* * */
    {0x08, 0x08, 0x3E, 0x08, 0x08}, /* + */
    {0x00, 0x50, 0x30, 0x00, 0x00}, /* , */
    {0x08, 0x08, 0x08, 0x08, 0x08}, /* - */
    {0x00, 0x60, 0x60, 0x00, 0x00}, /* . */
    {0x20, 0x10, 0x08, 0x04, 0x02}, /* / */
    {0x3E, 0x51, 0x49, 0x45, 0x3E}, /* 0 */
    {0x00, 0x42, 0x7F, 0x40, 0x00}, /* 1 */
    {0x42, 0x61, 0x51, 0x49, 0x46}, /* 2 */
    {0x21, 0x41, 0x45, 0x4B, 0x31}, /* 3 */
    {0x18, 0x14, 0x12, 0x7F, 0x10}, /* 4 */
    {0x27, 0x45, 0x45, 0x45, 0x39}, /* 5 */
    {0x3C, 0x4A, 0x49, 0x49, 0x30}, /* 6 */
    {0x01, 0x71, 0x09, 0x05, 0x03}, /* 7 */
    {0x36, 0x49, 0x49, 0x49, 0x36}, /* 8 */
    {0x06, 0x49, 0x49, 0x29, 0x1E}, /* 9 */
    {0x00, 0x36, 0x36, 0x00, 0x00}, /* : */
    {0x00, 0x56, 0x36, 0x00, 0x00}, /* ; */
    {0x00, 0x08, 0x14, 0x22, 0x41}, /* < */
    {0x14, 0x14, 0x14, 0x14, 0x14}, /* = */
    {0x41, 0x22, 0x14, 0x08, 0x00}, /* > */
    {0x02, 0x01, 0x51, 0x09, 0x06}, /* ? */
    {0x32, 0x49, 0x79, 0x41, 0x3E}, /* @ */
    {0x7E, 0x11, 0x11, 0x11, 0x7E}, /* A */
    {0x7F, 0x49, 0x49, 0x49, 0x36}, /* B */
    {0x3E, 0x41, 0x41, 0x41, 0x22}, /* C */
    {0x7F, 0x41, 0x41, 0x22, 0x1C}, /* D */
    {0x7F, 0x49, 0x49, 0x49, 0x41}, /* E */
    {0x7F, 0x09, 0x09, 0x09, 0x01}, /* F */
    {0x3E, 0x41, 0x49, 0x49, 0x7A}, /* G */
    {0x7F, 0x08, 0x08, 0x08, 0x7F}, /* H */
    {0x00, 0x41, 0x7F, 0x41, 0x00}, /* I */
    {0x20, 0x40, 0x41, 0x3F, 0x01}, /* J */
    {0x7F, 0x08, 0x14, 0x22, 0x41}, /* K */
    {0x7F, 0x40, 0x40, 0x40, 0x40}, /* L */
    {0x7F, 0x02, 0x0C, 0x02, 0x7F}, /* M */
    {0x7F, 0x04, 0x08, 0x10, 0x7F}, /* N */
    {0x3E, 0x41, 0x41, 0x41, 0x3E}, /* O */
    {0x7F, 0x09, 0x09, 0x09, 0x06}, /* P */
    {0x3E, 0x41, 0x51, 0x21, 0x5E}, /* Q */
    {0x7F, 0x09, 0x19, 0x29, 0x46}, /* R */
    {0x46, 0x49, 0x49, 0x49, 0x31}, /* S */
    {0x01, 0x01, 0x7F, 0x01, 0x01}, /* T */
    {0x3F, 0x40, 0x40, 0x40, 0x3F}, /* U */
    {0x1F, 0x20, 0x40, 0x20, 0x1F}, /* V */
    {0x3F, 0x40, 0x38, 0x40, 0x3F}, /* W */
    {0x63, 0x14, 0x08, 0x14, 0x63}, /* X */
    {0x07, 0x08, 0x70, 0x08, 0x07}, /* Y */
    {0x61, 0x51, 0x49, 0x45, 0x43}, /* Z */
    {0x00, 0x7F, 0x41, 0x41, 0x00}, /* [ */
    {0x02, 0x04, 0x08, 0x10, 0x20}, /* backslash */
    {0x00, 0x41, 0x41, 0x7F, 0x00}, /* ] */
    {0x04, 0x02, 0x01, 0x02, 0x04}, /* ^ */
    {0x40, 0x40, 0x40, 0x40, 0x40}, /* _ */
    {0x00, 0x01, 0x02, 0x04, 0x00}, /* ` */
    {0x20, 0x54, 0x54, 0x54, 0x78}, /* a */
    {0x7F, 0x48, 0x44, 0x44, 0x38}, /* b */
    {0x38, 0x44, 0x44, 0x44, 0x20}, /* c */
    {0x38, 0x44, 0x44, 0x48, 0x7F}, /* d */
    {0x38, 0x54, 0x54, 0x54, 0x18}, /* e */
    {0x08, 0x7E, 0x09, 0x01, 0x02}, /* f */
    {0x0C, 0x52, 0x52, 0x52, 0x3E}, /* g */
    {0x7F, 0x08, 0x04, 0x04, 0x78}, /* h */
    {0x00, 0x44, 0x7D, 0x40, 0x00}, /* i */
    {0x20, 0x40, 0x44, 0x3D, 0x00}, /* j */
    {0x7F, 0x10, 0x28, 0x44, 0x00}, /* k */
    {0x00, 0x41, 0x7F, 0x40, 0x00}, /* l */
    {0x7C, 0x04, 0x18, 0x04, 0x78}, /* m */
    {0x7C, 0x08, 0x04, 0x04, 0x78}, /* n */
    {0x38, 0x44, 0x44, 0x44, 0x38}, /* o */
    {0x7C, 0x14, 0x14, 0x14, 0x08}, /* p */
    {0x08, 0x14, 0x14, 0x18, 0x7C}, /* q */
    {0x7C, 0x08, 0x04, 0x04, 0x08}, /* r */
    {0x48, 0x54, 0x54, 0x54, 0x20}, /* s */
    {0x04, 0x3F, 0x44, 0x40, 0x20}, /* t */
    {0x3C, 0x40, 0x40, 0x20, 0x7C}, /* u */
    {0x1C, 0x20, 0x40, 0x20, 0x1C}, /* v */
    {0x3C, 0x40, 0x30, 0x40, 0x3C}, /* w */
    {0x44, 0x28, 0x10, 0x28, 0x44}, /* x */
    {0x0C, 0x50, 0x50, 0x50, 0x3C}, /* y */
    {0x44, 0x64, 0x54, 0x4C, 0x44}, /* z */
    {0x00, 0x08, 0x36, 0x41, 0x00}, /* { */
    {0x00, 0x00, 0x7F, 0x00, 0x00}, /* | */
    {0x00, 0x41, 0x36, 0x08, 0x00}, /* } */
    {0x02, 0x01, 0x02, 0x04, 0x02}, /* ~ */
};

/*
 * Beyond ASCII: the accented characters French needs, plus the punctuation a
 * model reaches for when writing it.
 *
 * Lowercase has room to spare — the letters sit in rows 2..6, leaving the top
 * two rows for an accent. Capitals fill rows 0..6, so the accented ones are
 * shifted down a row and marked in the single row that frees up. That mark can
 * only be one pixel tall, so É and È differ by which columns it covers rather
 * than by its slope. Legible, not beautiful; the alternative was dropping the
 * accent entirely, which changes the word.
 */
struct glyph_entry {
    uint32_t codepoint;
    uint8_t bits[FONT_GLYPH_W];
};

static const struct glyph_entry EXTRA_GLYPHS[] = {
    /* Lowercase, accent in the two free rows above the letter. */
    {0x00E0, {0x20, 0x55, 0x56, 0x54, 0x78}}, /* à */
    {0x00E2, {0x20, 0x56, 0x55, 0x56, 0x78}}, /* â */
    {0x00E4, {0x20, 0x56, 0x54, 0x56, 0x78}}, /* ä */
    {0x00E7, {0x38, 0x44, 0xC4, 0xC4, 0x20}}, /* ç — cedilla hangs in row 7 */
    {0x00E8, {0x38, 0x55, 0x56, 0x54, 0x18}}, /* è */
    {0x00E9, {0x38, 0x54, 0x56, 0x55, 0x18}}, /* é */
    {0x00EA, {0x38, 0x56, 0x55, 0x56, 0x18}}, /* ê */
    {0x00EB, {0x38, 0x56, 0x54, 0x56, 0x18}}, /* ë */
    {0x00EE, {0x00, 0x46, 0x7D, 0x42, 0x00}}, /* î */
    {0x00EF, {0x00, 0x45, 0x7C, 0x41, 0x00}}, /* ï */
    {0x00F4, {0x38, 0x46, 0x45, 0x46, 0x38}}, /* ô */
    {0x00F6, {0x38, 0x46, 0x44, 0x46, 0x38}}, /* ö */
    {0x00F9, {0x3C, 0x41, 0x42, 0x20, 0x7C}}, /* ù */
    {0x00FB, {0x3C, 0x42, 0x41, 0x22, 0x7C}}, /* û */
    {0x00FC, {0x3C, 0x42, 0x40, 0x22, 0x7C}}, /* ü */
    {0x00FF, {0x0C, 0x52, 0x50, 0x52, 0x3C}}, /* ÿ */
    {0x0153, {0x38, 0x44, 0x7C, 0x54, 0x18}}, /* œ — a squeeze at this size */

    /* Capitals, letter shifted into rows 1..7 with the mark in row 0. */
    {0x00C0, {0xFC, 0x23, 0x23, 0x22, 0xFC}}, /* À */
    {0x00C2, {0xFC, 0x23, 0x23, 0x23, 0xFC}}, /* Â */
    {0x00C4, {0xFC, 0x23, 0x22, 0x23, 0xFC}}, /* Ä */
    {0x00C7, {0x3E, 0x41, 0xC1, 0xC1, 0x22}}, /* Ç — room below, so no shift */
    {0x00C8, {0xFE, 0x93, 0x93, 0x92, 0x82}}, /* È */
    {0x00C9, {0xFE, 0x92, 0x93, 0x93, 0x82}}, /* É */
    {0x00CA, {0xFE, 0x93, 0x93, 0x93, 0x82}}, /* Ê */
    {0x00CB, {0xFE, 0x93, 0x92, 0x93, 0x82}}, /* Ë */
    {0x00CE, {0x00, 0x83, 0xFF, 0x83, 0x00}}, /* Î */
    {0x00CF, {0x00, 0x83, 0xFE, 0x83, 0x00}}, /* Ï */
    {0x00D4, {0x7C, 0x83, 0x83, 0x83, 0x7C}}, /* Ô */
    {0x00D6, {0x7C, 0x83, 0x82, 0x83, 0x7C}}, /* Ö */
    {0x00D9, {0x7E, 0x81, 0x81, 0x80, 0x7E}}, /* Ù */
    {0x00DB, {0x7E, 0x81, 0x81, 0x81, 0x7E}}, /* Û */
    {0x00DC, {0x7E, 0x81, 0x80, 0x81, 0x7E}}, /* Ü */
    {0x0152, {0x7C, 0x82, 0xFE, 0x92, 0x82}}, /* Œ */

    /* Punctuation a model writing French will reach for. Without these the
     * text is peppered with '?' at every quote and apostrophe. */
    {0x00AB, {0x00, 0x08, 0x14, 0x22, 0x41}}, /* « */
    {0x00BB, {0x41, 0x22, 0x14, 0x08, 0x00}}, /* » */
    {0x00B0, {0x00, 0x06, 0x09, 0x09, 0x06}}, /* ° */
    {0x2018, {0x00, 0x03, 0x05, 0x00, 0x00}}, /* ' */
    {0x2019, {0x00, 0x05, 0x03, 0x00, 0x00}}, /* ' */
    {0x201C, {0x00, 0x07, 0x00, 0x07, 0x00}}, /* " */
    {0x201D, {0x00, 0x07, 0x00, 0x07, 0x00}}, /* " */
    {0x2013, {0x08, 0x08, 0x08, 0x08, 0x08}}, /* – */
    {0x2014, {0x08, 0x08, 0x08, 0x08, 0x08}}, /* — */
    {0x2026, {0x40, 0x00, 0x40, 0x00, 0x40}}, /* … */
    {0x20AC, {0x14, 0x3E, 0x55, 0x41, 0x22}}, /* € */
};

/* Resolves a codepoint to a glyph, falling back to '?' so an unsupported
 * character is visibly wrong rather than silently missing. */
static const uint8_t *glyph_for(uint32_t codepoint)
{
    if (codepoint >= FONT_FIRST_CHAR && codepoint <= FONT_LAST_CHAR) {
        return FONT_5X7[codepoint - FONT_FIRST_CHAR];
    }

    for (size_t i = 0; i < sizeof EXTRA_GLYPHS / sizeof EXTRA_GLYPHS[0]; i++) {
        if (EXTRA_GLYPHS[i].codepoint == codepoint) {
            return EXTRA_GLYPHS[i].bits;
        }
    }

    return FONT_5X7['?' - FONT_FIRST_CHAR];
}

/**
 * Reads one UTF-8 character, returning how many bytes it used.
 *
 * The app sends UTF-8, so anything above ASCII arrives as two or more bytes.
 * Treating those bytes individually — which is what the panel did before — put
 * two '?' on screen for every accented letter.
 *
 * A malformed sequence consumes one byte and yields '?', so bad input costs a
 * character rather than desynchronising everything after it.
 */
static size_t decode_utf8(const char *text, uint32_t *codepoint)
{
    const unsigned char *p = (const unsigned char *)text;

    if (p[0] < 0x80) {
        *codepoint = p[0];
        return 1;
    }
    if ((p[0] & 0xE0) == 0xC0 && (p[1] & 0xC0) == 0x80) {
        *codepoint = ((uint32_t)(p[0] & 0x1F) << 6) | (p[1] & 0x3F);
        return 2;
    }
    if ((p[0] & 0xF0) == 0xE0 && (p[1] & 0xC0) == 0x80 && (p[2] & 0xC0) == 0x80) {
        *codepoint = ((uint32_t)(p[0] & 0x0F) << 12) |
                     ((uint32_t)(p[1] & 0x3F) << 6) | (p[2] & 0x3F);
        return 3;
    }
    if ((p[0] & 0xF8) == 0xF0 && (p[1] & 0xC0) == 0x80 &&
        (p[2] & 0xC0) == 0x80 && (p[3] & 0xC0) == 0x80) {
        *codepoint = ((uint32_t)(p[0] & 0x07) << 18) |
                     ((uint32_t)(p[1] & 0x3F) << 12) |
                     ((uint32_t)(p[2] & 0x3F) << 6) | (p[3] & 0x3F);
        return 4;
    }

    *codepoint = '?';
    return 1;
}

/* True for anything that separates words. U+00A0 is here because French
 * typography puts a non-breaking space before ! ? : and », and a model writing
 * French emits it — as a glyph it would be a '?' in the middle of every
 * sentence. */
static bool is_space(uint32_t codepoint)
{
    return codepoint == ' ' || codepoint == 0x00A0;
}

/* Blits one glyph at a character cell. */
static void draw_glyph(int col, int row, const uint8_t *glyph)
{
    int x = col * DISPLAY_CELL_W;

    for (int i = 0; i < FONT_GLYPH_W; i++) {
        s_framebuffer[row * DISPLAY_WIDTH + x + i] = glyph[i];
    }
    /* The spacing column. */
    s_framebuffer[row * DISPLAY_WIDTH + x + FONT_GLYPH_W] = 0x00;
}

static void flush(void)
{
    if (s_panel == NULL) {
        return;
    }
    esp_lcd_panel_draw_bitmap(s_panel, 0, 0, DISPLAY_WIDTH, DISPLAY_HEIGHT,
                              s_framebuffer);
}

/* How many cells the word starting at `text` needs. Counted in characters, not
 * bytes: an accented word is shorter on screen than it is in memory, and
 * measuring it in bytes would wrap it a column early. */
static int word_cells(const char *text)
{
    int cells = 0;

    while (*text != '\0') {
        uint32_t codepoint;
        size_t used = decode_utf8(text, &codepoint);

        if (is_space(codepoint) || codepoint == '\n') {
            break;
        }
        cells++;
        text += used;
    }
    return cells;
}

/**
 * Draws one screenful starting at `text`.
 *
 * Returns how many bytes were consumed, which is where the next page begins.
 * Pagination has to live here rather than in the app: a page boundary is a
 * line boundary, and only this function knows where the lines break.
 */
static size_t render(const char *text)
{
    const char *start = text;
    int col = 0;
    int row = 0;

    memset(s_framebuffer, 0, sizeof s_framebuffer);

    if (text == NULL) {
        flush();
        return 0;
    }

    while (*text != '\0' && row < DISPLAY_ROWS) {
        uint32_t codepoint;
        size_t used = decode_utf8(text, &codepoint);

        if (codepoint == '\n') {
            col = 0;
            row++;
            text += used;
            continue;
        }

        if (is_space(codepoint)) {
            /* A space that would start a line is swallowed, so wrapped text
             * does not begin with a gap. */
            if (col > 0 && col < DISPLAY_COLS) {
                draw_glyph(col, row, glyph_for(' '));
                col++;
            }
            text += used;
            continue;
        }

        /* Move the whole word down if it fits on a line of its own; a word
         * longer than the panel is broken mid-way instead, which beats
         * dropping it. */
        int word = word_cells(text);
        if (col > 0 && col + word > DISPLAY_COLS && word <= DISPLAY_COLS) {
            col = 0;
            row++;
            continue;
        }

        if (col >= DISPLAY_COLS) {
            col = 0;
            row++;
            continue;
        }

        draw_glyph(col, row, glyph_for(codepoint));
        col++;
        text += used;
    }

    flush();
    return (size_t)(text - start);
}

/*
 * The text currently on screen. `append` adds to it, which is how streaming
 * tokens will arrive; when it fills, the oldest text is dropped rather than
 * the newest, so the panel behaves like a teleprompter rather than freezing
 * on the first screenful.
 */
static char s_text[DISPLAY_TEXT_BUFFER];
static size_t s_text_len;

/* Where the visible page starts in s_text, and how much of it is on screen.
 * Together they say what has been read and what is still waiting. */
static size_t s_page_start;
static size_t s_page_len;

static void show_page(void)
{
    s_page_len = render(s_text + s_page_start);
}

/* True when the response runs past the bottom of the current page. */
static bool more_to_show(void)
{
    return s_page_start + s_page_len < s_text_len;
}

static TickType_t page_hold_ticks(void)
{
    uint32_t ms = PAGE_HOLD_BASE_MS + s_page_len * PAGE_HOLD_PER_CHAR_MS;

    if (ms > PAGE_HOLD_MAX_MS) {
        ms = PAGE_HOLD_MAX_MS;
    }
    return pdMS_TO_TICKS(ms);
}

static void advance_page(void)
{
    if (!more_to_show()) {
        return;
    }

    /* A page that draws nothing while text remains would leave the cursor
     * stuck here forever; step over a byte so it always makes progress. */
    s_page_start += s_page_len > 0 ? s_page_len : 1;
    show_page();
}

static void apply(const struct display_msg *msg)
{
    /* Appending only changes the screen when the last page is the visible one.
     * While the reader is still on an earlier page, new tokens land out of
     * sight and redrawing would only cost I2C and flicker. */
    bool showing_tail = s_page_start + s_page_len >= s_text_len;

    switch (msg->op) {
    case DISPLAY_OP_CLEAR:
        s_text_len = 0;
        s_text[0] = '\0';
        s_page_start = 0;
        break;

    case DISPLAY_OP_SET:
        s_text_len = msg->len < sizeof s_text - 1 ? msg->len : sizeof s_text - 1;
        memcpy(s_text, msg->text, s_text_len);
        s_text[s_text_len] = '\0';
        /* A new response starts at the top. */
        s_page_start = 0;
        break;

    case DISPLAY_OP_APPEND:
        if (s_text_len + msg->len > sizeof s_text - 1) {
            /* Keep the tail: drop enough from the front to fit what arrived. */
            size_t overflow = s_text_len + msg->len - (sizeof s_text - 1);
            if (overflow >= s_text_len) {
                s_text_len = 0;
                s_page_start = 0;
            } else {
                memmove(s_text, s_text + overflow, s_text_len - overflow);
                s_text_len -= overflow;
                /* Everything shifted down by `overflow`; move the reader's
                 * place with it rather than jumping them forward. */
                s_page_start = s_page_start > overflow ? s_page_start - overflow : 0;
            }
        }
        size_t room = sizeof s_text - 1 - s_text_len;
        size_t take = msg->len < room ? msg->len : room;
        memcpy(s_text + s_text_len, msg->text, take);
        s_text_len += take;
        s_text[s_text_len] = '\0';

        if (!showing_tail) {
            return;     /* out of view; the page timer will get to it */
        }
        break;

    default:
        ESP_LOGW(TAG, "unknown display op %u", msg->op);
        return;
    }

    show_page();
}

static void display_task(void *arg)
{
    struct display_msg msg;

    for (;;) {
        /* Block indefinitely when everything fits — the panel is finished, and
         * only a new write has anything to say. Otherwise wake up to turn the
         * page. */
        TickType_t wait = more_to_show() ? page_hold_ticks() : portMAX_DELAY;

        if (xQueueReceive(s_queue, &msg, wait) == pdTRUE) {
            apply(&msg);
        } else {
            advance_page();
        }
    }
}

bool display_post(uint8_t op, const char *text, size_t len)
{
    struct display_msg msg = { .op = op };

    if (s_queue == NULL) {
        return false;
    }
    if (len > DISPLAY_TEXT_MAX) {
        len = DISPLAY_TEXT_MAX;
    }
    if (text != NULL && len > 0) {
        memcpy(msg.text, text, len);
    }
    msg.len = (uint16_t)len;

    /* Never block: this is called from the BLE host task, and a stalled panel
     * must not stall the radio. */
    return xQueueSend(s_queue, &msg, 0) == pdTRUE;
}

void display_show(const char *text)
{
    display_post(DISPLAY_OP_SET, text, text != NULL ? strlen(text) : 0);
}

void display_clear(void)
{
    display_post(DISPLAY_OP_CLEAR, NULL, 0);
}

void display_show_idle(bool app_connected)
{
    /* Deliberately not blank. A dark panel is indistinguishable from a device
     * that has crashed or ignored you, and this is the wearer's only output.
     * Placeholder wording until the panel gets a real UI. */
    display_show(app_connected ? "minicole\nconnected"
                               : "minicole\nconnect to app to continue");
}

esp_err_t display_init(void)
{
    i2c_master_bus_config_t bus_config = {
        .i2c_port = -1,                     /* let the driver pick a port */
        .sda_io_num = DISPLAY_SDA_GPIO,
        .scl_io_num = DISPLAY_SCL_GPIO,
        .clk_source = I2C_CLK_SRC_DEFAULT,
        .glitch_ignore_cnt = 7,
        .flags.enable_internal_pullup = true,
    };
    i2c_master_bus_handle_t bus = NULL;
    esp_err_t err;

    err = i2c_new_master_bus(&bus_config, &bus);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "i2c bus init failed; err=%d", err);
        return err;
    }

    uint32_t address = DISPLAY_ADDR_PRIMARY;
    if (i2c_master_probe(bus, DISPLAY_ADDR_PRIMARY, DISPLAY_PROBE_TIMEOUT_MS)
        != ESP_OK) {
        if (i2c_master_probe(bus, DISPLAY_ADDR_ALTERNATE,
                             DISPLAY_PROBE_TIMEOUT_MS) == ESP_OK) {
            address = DISPLAY_ADDR_ALTERNATE;
        } else {
            ESP_LOGE(TAG, "no display at 0x%02X or 0x%02X — check SDA on GPIO%d, "
                          "SCL on GPIO%d, and 3V3/GND",
                     DISPLAY_ADDR_PRIMARY, DISPLAY_ADDR_ALTERNATE,
                     DISPLAY_SDA_GPIO, DISPLAY_SCL_GPIO);
            return ESP_ERR_NOT_FOUND;
        }
    }
    ESP_LOGI(TAG, "display found at 0x%02X", (unsigned)address);

    esp_lcd_panel_io_i2c_config_t io_config = {
        .dev_addr = address,
        .scl_speed_hz = DISPLAY_I2C_HZ,
        .control_phase_bytes = 1,
        .dc_bit_offset = 6,
        .lcd_cmd_bits = 8,
        .lcd_param_bits = 8,
    };
    esp_lcd_panel_io_handle_t io = NULL;
    err = esp_lcd_new_panel_io_i2c(bus, &io_config, &io);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "panel io init failed; err=%d", err);
        return err;
    }

    esp_lcd_panel_ssd1306_config_t ssd1306_config = {
        .height = DISPLAY_HEIGHT,
    };
    esp_lcd_panel_dev_config_t panel_config = {
        .bits_per_pixel = 1,
        .reset_gpio_num = -1,               /* the module resets itself */
        .vendor_config = &ssd1306_config,
    };
    err = esp_lcd_new_panel_ssd1306(io, &panel_config, &s_panel);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "panel init failed; err=%d", err);
        s_panel = NULL;
        return err;
    }

    ESP_ERROR_CHECK(esp_lcd_panel_reset(s_panel));
    ESP_ERROR_CHECK(esp_lcd_panel_init(s_panel));
    ESP_ERROR_CHECK(esp_lcd_panel_disp_on_off(s_panel, true));

    s_queue = xQueueCreate(DISPLAY_QUEUE_DEPTH, sizeof(struct display_msg));
    if (s_queue == NULL) {
        ESP_LOGE(TAG, "could not create the display queue");
        return ESP_ERR_NO_MEM;
    }
    if (xTaskCreate(display_task, "monocle_display", 4096, NULL, 4, NULL)
        != pdPASS) {
        ESP_LOGE(TAG, "could not start the display task");
        vQueueDelete(s_queue);
        s_queue = NULL;
        return ESP_ERR_NO_MEM;
    }

    display_clear();
    ESP_LOGI(TAG, "display ready (%dx%d, %d cols x %d rows)",
             DISPLAY_WIDTH, DISPLAY_HEIGHT, DISPLAY_COLS, DISPLAY_ROWS);
    return ESP_OK;
}
