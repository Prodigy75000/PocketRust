; SPDX-License-Identifier: CC0-1.0
; PocketRust Demo Cart. Dedicated to the public domain; see LICENSE.
;
; Screens, palettes, and music. Everything here is data the code in main.s
; reads; there are no instructions in this file.

; ---- tile indices --------------------------------------------------------
; The font occupies $00-$3F (character code minus $20), so the graphics start
; at $40. tiles.s asserts that it still does.

T_SOLID1  = $40
T_SOLID2  = $41
T_SOLID3  = $42
T_CURSOR  = $43
T_ORB     = $44
T_GEM     = $45
T_BRICK_A = $46
T_BRICK_B = $47
T_STAR    = $48
T_CLOUD   = $49
T_SWATCH  = $4a
T_BAR0    = $4b      ; and $4c, $4d, $4e: four heights, T_BAR0 plus the level
T_KEY_OFF = $4f
T_KEY_ON  = $50

; ---- screen scripts ------------------------------------------------------
; Each run is a destination address, a length, then that many tile indices.
; A zero destination ends the script. `.str` subtracts $20 from every
; character, which is exactly the font's layout, so screen text is written as
; text.

screen_menu:
  .word SCRN + 32*1 + 5
  .byte 10
  .str "POCKETRUST"
  .word SCRN + 32*2 + 3
  .byte 14
  .str "DEMO CART V1.0"
  .word SCRN + 32*4 + 0
  .byte 20
  .str "--------------------"
  .word SCRN + 32*6 + 3
  .byte 9
  .str "1 SPRITES"
  .word SCRN + 32*7 + 3
  .byte 14
  .str "2 SCROLL SPLIT"
  .word SCRN + 32*8 + 3
  .byte 8
  .str "3 COLORS"
  .word SCRN + 32*9 + 3
  .byte 7
  .str "4 AUDIO"
  .word SCRN + 32*10 + 3
  .byte 7
  .str "5 INPUT"
  .word SCRN + 32*12 + 1
  .byte 17
  .str "UP DOWN TO CHOOSE"
  .word SCRN + 32*13 + 1
  .byte 19
  .str "A ENTERS  B RETURNS"
  .word SCRN + 32*15 + 1
  .byte 17
  .str "CC0 PUBLIC DOMAIN"
  .word SCRN + 32*16 + 0
  .byte 19
  .str "GITHUB/PRODIGY75000"
  .word SCRN + 32*17 + 0
  .byte 5
  .str "MODE:"
  .word 0

screen_sprites:
  .word SCRN + 32*0 + 0
  .byte 20
  .str "1 SPRITES  40 OBJS  "
  .word SCRN + 32*1 + 0
  .byte 20
  .str "A ARRANGEMENT       "
  .word SCRN + 32*2 + 0
  .byte 20
  .str "SELECT 8X16  B BACK "
  .word 0

screen_scroll:
  .word SCRN + 32*0 + 0
  .byte 20
  .str "2 SCROLL SPLIT      "
  .word SCRN + 32*1 + 0
  .byte 20
  .str "LEFT RIGHT SPEED    "
  .word SCRN + 32*2 + 0
  .byte 20
  .str "A SPLIT ON OFF      "
  .word 0

screen_colors:
  .word SCRN + 32*0 + 0
  .byte 20
  .str "3 COLORS            "
  .word SCRN + 32*13 + 1
  .byte 6
  .str "LEVEL:"
  .word SCRN + 32*15 + 0
  .byte 20
  .str "LEFT RIGHT STEP BY 1"
  .word SCRN + 32*16 + 0
  .byte 20
  .str "UP DOWN STEP BY 8   "
  .word SCRN + 32*17 + 0
  .byte 20
  .str "B BACK              "
  .word 0

; The seven swatch labels, drawn under each block: which channels the block
; lights. On a monochrome Game Boy only the first four are used, and they name
; the four shades instead.
swatch_labels:
  .str "R "
  .str "G "
  .str "B "
  .str "RG"
  .str "RB"
  .str "GB"
  .str "W "
.assert swatch_labels_end - swatch_labels == 14, "seven swatches, two characters each"
swatch_labels_end:

shade_labels:
  .str "0 "
  .str "1 "
  .str "2 "
  .str "3 "

screen_audio:
  .word SCRN + 32*0 + 0
  .byte 20
  .str "4 AUDIO             "
  .word SCRN + 32*2 + 2
  .byte 18
  .str "CHANNEL      STATE"
  .word SCRN + 32*4 + 2
  .byte 9
  .str "1 PULSE A"
  .word SCRN + 32*6 + 2
  .byte 9
  .str "2 PULSE B"
  .word SCRN + 32*8 + 2
  .byte 9
  .str "3 WAVE   "
  .word SCRN + 32*10 + 2
  .byte 9
  .str "4 NOISE  "
  .word SCRN + 32*13 + 0
  .byte 20
  .str "UP DOWN PICK        "
  .word SCRN + 32*14 + 0
  .byte 20
  .str "A MUTES A CHANNEL   "
  .word SCRN + 32*15 + 0
  .byte 20
  .str "START RESTARTS THE  "
  .word SCRN + 32*16 + 0
  .byte 20
  .str "PATTERN             "
  .word SCRN + 32*17 + 0
  .byte 20
  .str "B BACK              "
  .word 0

; The two words the state column shows. Four characters each, so the longer one
; always covers the shorter one and the column never needs clearing.
str_on:
  .str "ON  "
str_mute:
  .str "MUTE"
.assert str_mute - str_on == 4, "the state words are four characters each"

screen_input:
  .word SCRN + 32*0 + 0
  .byte 20
  .str "5 INPUT             "
  .word SCRN + 32*3 + 1
  .byte 5
  .str "D PAD"
  .word SCRN + 32*9 + 1
  .byte 7
  .str "BUTTONS"
  .word SCRN + 32*10 + 1
  .byte 18
  .str "A  B  SELECT START"
  .word SCRN + 32*13 + 1
  .byte 9
  .str "DPAD RAW:"
  .word SCRN + 32*14 + 1
  .byte 9
  .str "BTN  RAW:"
  .word SCRN + 32*15 + 1
  .byte 7
  .str "FRAMES:"
  .word SCRN + 32*17 + 0
  .byte 20
  .str "B BACK              "
  .word 0

; The scroll screen's status bar is drawn into map rows 0-2. Row 3 is left
; blank on purpose, so the last scanline above the raster split is a flat
; colour; SCROLL_BAR_ROWS in main.s is what puts the split there.


; ---- colour ---------------------------------------------------------------
; Game Boy Color palettes are five bits per channel, little endian:
; %0BBBBBGG %GGGRRRRR. Palette 0 is the text palette and every screen keeps it,
; so the swatches on the colour screen use palettes 1 through 7.

; Palette 0: black text on white, with two greys between, which is what the
; monochrome shades look like on a colour screen.
pal_text:
  .word $7fff, $52aa, $2529, $0000

; The scroll screen's sky, and its bricks. The sky's colour 0 is the sky itself,
; because the tile behind it is the blank one; its colour 2 is what the stars
; and the clouds are drawn in.
pal_sky:
  .word $7e8c, $6318, $7fff, $0000
pal_brick:
  .word $7fff, $1916, $0c8c, $0888

; The object screen: the orbs and the gems get their own object palettes so the
; two rings are told apart by colour and not only by shape.
pal_obj_orb:
  .word $7fff, $7fe0, $1cbf, $0000
pal_obj_gem:
  .word $7fff, $7c1f, $4008, $0000

; ---- music ---------------------------------------------------------------
; Sixteen steps. $ff is a rest, anything else indexes note_table in tables.s.
; The two pulse channels play a line and its answer, the wave channel holds the
; root down an octave, and the noise channel keeps time.

REST = $ff

music_ch1:
  .byte NOTE_A4, REST,     NOTE_C5,  REST,     NOTE_E5,  REST,     NOTE_C5,  REST
  .byte NOTE_G4, REST,     NOTE_B4,  REST,     NOTE_D5,  REST,     NOTE_B4,  REST

music_ch2:
  .byte REST,    NOTE_E4,  REST,     NOTE_A4,  REST,     NOTE_E4,  REST,     NOTE_A4
  .byte REST,    NOTE_D4,  REST,     NOTE_G4,  REST,     NOTE_D4,  REST,     NOTE_G4

music_ch3:
  .byte NOTE_A3, REST,     REST,     REST,     NOTE_A3,  REST,     REST,     REST
  .byte NOTE_G3, REST,     REST,     REST,     NOTE_G3,  REST,     REST,     REST

; The noise channel is programmed with a shift and a divisor rather than a
; note, so these are NR43 values: a bright tick on the beat, a duller one off it.
music_ch4:
  .byte $52,     REST,     $37,      REST,     $52,      REST,     $37,      $37
  .byte $52,     REST,     $37,      REST,     $52,      REST,     $37,      $37

.assert music_ch2 - music_ch1 == 16, "the music patterns are 16 steps each"
.assert music_ch3 - music_ch2 == 16, "the music patterns are 16 steps each"
.assert music_ch4 - music_ch3 == 16, "the music patterns are 16 steps each"

; The wave channel's 32 four-bit samples, two per byte. A rounded ramp: it has
; more in it than a square and less than a sine, which is audible on a channel
; that has no envelope of its own.
wave_pattern:
  .byte $02, $46, $8a, $cd, $ef, $ff, $fe, $dc
  .byte $ba, $98, $76, $54, $32, $10, $00, $01

.assert wave_pattern_end - wave_pattern == 16, "wave RAM is sixteen bytes"
wave_pattern_end:

; ---- input screen key positions ------------------------------------------
; One entry per button, in the order the joypad byte reports them: A, B,
; Select, Start, Right, Left, Up, Down. Each is the map address of the cell the
; indicator is drawn in.

key_cells:
  .word SCRN + 32*11 + 1     ; A
  .word SCRN + 32*11 + 4     ; B
  .word SCRN + 32*11 + 9     ; Select
  .word SCRN + 32*11 + 16    ; Start
  .word SCRN + 32*5  + 4     ; Right
  .word SCRN + 32*5  + 2     ; Left
  .word SCRN + 32*4  + 3     ; Up
  .word SCRN + 32*6  + 3     ; Down

.assert key_cells_end - key_cells == 16, "eight buttons, two bytes each"
key_cells_end:
