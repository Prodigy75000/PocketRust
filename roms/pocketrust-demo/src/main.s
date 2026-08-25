; SPDX-License-Identifier: CC0-1.0
; PocketRust Demo Cart. Dedicated to the public domain; see LICENSE.
;
; A Game Boy Color cartridge that is not a game. It is five screens, each of
; which puts one part of the machine under load so that a person looking at the
; screen can tell whether the emulator got it right.
;
; Assembled by gb-asm, in this repository. Nothing outside this repository is
; needed to turn this file into the .gbc beside it.

; ---- hardware ------------------------------------------------------------

P1    = $ff00        ; joypad: write a selector, read the four lines back
DIV   = $ff04
IF    = $ff0f

NR10  = $ff10        ; pulse A: sweep, duty, envelope, period, control
NR11  = $ff11
NR12  = $ff12
NR13  = $ff13
NR14  = $ff14
NR21  = $ff16        ; pulse B (no sweep, so no NR20)
NR22  = $ff17
NR23  = $ff18
NR24  = $ff19
NR30  = $ff1a        ; wave: DAC, length, level, period, control
NR31  = $ff1b
NR32  = $ff1c
NR33  = $ff1d
NR34  = $ff1e
NR41  = $ff20        ; noise: length, envelope, shift/divisor, control
NR42  = $ff21
NR43  = $ff22
NR44  = $ff23
NR50  = $ff24        ; master volume, panning, power
NR51  = $ff25
NR52  = $ff26
WAVERAM = $ff30      ; sixteen bytes, thirty-two four-bit samples

LCDC  = $ff40
STAT  = $ff41
SCY   = $ff42
SCX   = $ff43
LY    = $ff44
LYC   = $ff45
DMA   = $ff46
BGP   = $ff47
OBP0  = $ff48
OBP1  = $ff49
WY    = $ff4a
WX    = $ff4b
VBK   = $ff4f        ; CGB only: which VRAM bank $8000-$9FFF shows
BCPS  = $ff68        ; CGB only: background palette index, and the data port
BCPD  = $ff69
OCPS  = $ff6a        ; CGB only: object palette index, and the data port
OCPD  = $ff6b
IE    = $ffff

VRAM  = $8000        ; tile data, 384 tiles
SCRN  = $9800        ; background map, 32 by 32

IE_VBLANK = %00000001
IE_STAT   = %00000010

; Where the object transfer routine is copied to and called from.
dma_start = $ff80

; The scroll screen's status bar is four tile rows tall, so the raster split has
; to happen on the last line of the fourth one.
SCROLL_BAR_ROWS = 4
SPLIT_LINE = SCROLL_BAR_ROWS * 8 - 1

; LCDC: on, window map at $9C00, window off, tile data at $8000, BG map at
; $9800, 8x8 objects, objects on, background on.
LCDCF_ON  = %10010011
LCDCF_BIG = %00000100    ; the 8x16 object bit the sprite screen toggles

; The joypad byte this cartridge builds: one bit per button, pressed = 1.
BTN_A      = %00000001
BTN_B      = %00000010
BTN_SELECT = %00000100
BTN_START  = %00001000
BTN_RIGHT  = %00010000
BTN_LEFT   = %00100000
BTN_UP     = %01000000
BTN_DOWN   = %10000000

QUEUE_MAX = 48       ; single-tile writes a scene may leave for the next VBlank
SCENE_NONE = $ff     ; wPending when no screen change is waiting

; ---- the cartridge -------------------------------------------------------
; No mapper and 32 KB, which is the widest-compatibility Game Boy cartridge
; there is: it is the shape a Game Boy addresses directly, with no bank
; switching hardware between the CPU and the ROM at all.
;
; cgb=on marks it as using colour where there is colour while still running on
; a monochrome Game Boy, which is why every screen here has something to say in
; both modes.

.gb title="POCKETRUST" cgb=on sgb=off mbc=none rom=32 version=0

; ---- work RAM ------------------------------------------------------------
; Addresses only. A cartridge cannot ship RAM contents, so nothing here emits a
; byte; the reset routine clears the lot.

.ram $c000
; The object attribute buffer. It has to start on a $100 boundary because the
; DMA controller takes only the high byte of the source address.
wOam:          .res 160

.ram $c100
; Single-tile background writes a scene queues during the frame, drained by the
; VBlank handler. Three bytes each: address low, address high, tile.
wQueue:        .res QUEUE_MAX * 3

.ram $c200
wVBlankDone:   .res 1    ; set by the VBlank handler, cleared by the main loop
wQueueLen:     .res 1    ; entries waiting in wQueue
wFrame:        .res 2    ; frames since reset, low byte first
wJoy:          .res 1    ; buttons held now
wJoyPrev:      .res 1
wJoyNew:       .res 1    ; buttons that went down this frame
wRawDpad:      .res 1    ; the four lines as the direction selector reports them
wRawBtn:       .res 1
wScene:        .res 1
wPending:      .res 1    ; screen to change to, or SCENE_NONE
wCursor:       .res 1    ; menu selection
wIsCgb:        .res 1
wLcdc:         .res 1    ; what rendering_on turns the screen back on with

wArrange:      .res 1    ; sprite screen: which of the four arrangements
wBig:          .res 1    ; sprite screen: 8x16 objects
wOamRot:       .res 1    ; sprite screen: which object owns the first OAM slot

wScrollX:      .res 1    ; scroll screen: where the playfield has got to
wSpeed:        .res 1    ; scroll screen: signed, -4 to 4
wSplit:        .res 1    ; scroll screen: is the status bar being held still

wLevel:        .res 1    ; colour screen: 0-31, the intensity the swatches show
wPalDirty:     .res 1    ; colour screen: rewrite the palettes next VBlank
wBlockIndex:   .res 1    ; colour screen: which swatch is being laid out

wChanSel:      .res 1    ; audio screen: 0-3
wMute:         .res 1    ; audio screen: one bit per channel
wStep:         .res 1    ; audio screen: position in the sixteen-step pattern
wTick:         .res 1    ; audio screen: frames until the next step

; ---- interrupt vectors ---------------------------------------------------

.org $0040
  jp vblank_isr

.org $0048
  jp stat_isr

.org $0050
  reti                   ; timer, unused

.org $0058
  reti                   ; serial, unused

.org $0060
  reti                   ; joypad, unused: this cartridge polls instead

; ---- header --------------------------------------------------------------

.org $0100
  nop
  jp start

.org $0104
; The 48 bytes at $0104 are the Game Boy's lock: the boot ROM compares them
; against its own copy and refuses to start the cartridge if they differ, so
; every cartridge that runs on the hardware carries them, and this one is no
; more or less original for having them. They are Nintendo's, they are the only
; bytes in this file that are, and PERMISSION.md says so in as many words.
  .byte $ce, $ed, $66, $66, $cc, $0d, $00, $0b, $03, $73, $00, $83, $00, $0c, $00, $0d
  .byte $00, $08, $11, $1f, $88, $89, $00, $0e, $dc, $cc, $6e, $e6, $dd, $dd, $d9, $99
  .byte $bb, $bb, $67, $63, $6e, $0e, $ec, $cc, $dd, $dc, $99, $9f, $bb, $b9, $33, $3e

.assert $0134 - $0104 == 48, "the logo field is 48 bytes"

; $0134 to $014F is the rest of the header: title, hardware flags, and the two
; checksums. gb-asm fills that in from the .gb line above and from the finished
; image, and refuses to assemble if anything here tries to write there.

; ---- reset ---------------------------------------------------------------

.org $0150

start:
  di
  ld sp, $fffe
  ; The boot ROM leaves $11 in the accumulator on a Game Boy Color and $01 on a
  ; monochrome one. This is the only moment that is readable, so it is read
  ; before anything else touches the accumulator.
  ld b, 0
  cp $11
  jr nz, @mono
  ld b, 1
@mono:
  call lcd_off
  ; The answer has to survive clearing work RAM, and clearing work RAM needs a
  ; sixteen-bit counter, which is b. The stack is in high RAM and is not being
  ; cleared, so it is the one place to put it.
  push bc
  call clear_wram
  pop bc
  ld a, b
  ld [wIsCgb], a

  call load_tiles
  call install_dma
  call silence_audio

  ; The monochrome palettes. On a colour machine these are ignored, but a
  ; cartridge marked cgb=on has to be honest on both, so they are set anyway.
  ld a, %11100100
  ldh [BGP], a
  ldh [OBP0], a
  ld a, %11010000
  ldh [OBP1], a

  xor a
  ldh [SCX], a
  ldh [SCY], a
  ldh [WY], a
  ld a, 7
  ldh [WX], a

  ld a, SCENE_NONE
  ld [wPending], a
  xor a
  ld [wScene], a
  call scene_enter

  ld a, IE_VBLANK
  ldh [IE], a
  xor a
  ldh [IF], a
  ei
  call rendering_on

main:
  call wait_frame
  call read_joypad
  call scene_tick
  ld a, [wPending]
  cp SCENE_NONE
  jr z, main
  call change_scene
  jr main

; ---- frame ---------------------------------------------------------------

; Block until the VBlank handler has run. Everything a scene does happens
; between one of these and the next, with the screen on.
wait_frame:
  ld a, [wVBlankDone]
  or a
  jr z, wait_frame
  xor a
  ld [wVBlankDone], a
  ret

vblank_isr:
  push af
  push bc
  push de
  push hl
  ; The object buffer goes first: it is the one transfer that has a deadline,
  ; and the routine that runs it lives in high RAM because the CPU cannot see
  ; anything else while the controller has the bus.
  ld a, >wOam
  call dma_start
  call drain_queue
  call scene_vbl
  ld hl, wFrame
  inc [hl]
  jr nz, @done
  inc hl
  inc [hl]
@done:
  ld a, 1
  ld [wVBlankDone], a
  pop hl
  pop de
  pop bc
  pop af
  reti

; The raster split. LYC is set to the last line of the status bar, so this fires
; while that line is still being drawn; waiting for the horizontal blank at the
; end of it is what makes the new scroll position take effect on the next line
; rather than halfway through this one.
stat_isr:
  push af
@wait:
  ldh a, [STAT]
  and %00000011
  jr nz, @wait
  ld a, [wScrollX]
  ldh [SCX], a
  pop af
  reti

; ---- screen on and off ---------------------------------------------------

; Turn the screen off, having first thrown away anything the outgoing scene
; queued. The scene whose B press triggered the change still finished its tick,
; so its writes are sitting in the queue with addresses that mean something on
; the screen being left and nothing on the one being entered.
;
; As the code stands the wait below always spans a vertical blank, so the queue
; is drained onto the old screen before the new one is drawn and this clear
; changes nothing that can be seen. It is here so that stays true of the queue
; rather than of the timing: a scene whose tick finished early enough to leave
; the queue full would otherwise scatter it across the next screen.
rendering_off:
  xor a
  ld [wQueueLen], a
  ; fall through

lcd_off:
  ldh a, [LCDC]
  bit 7, a
  ret z
  ; Wait until the screen is in its vertical blank before switching it off,
  ; which is the only moment that is safe on real hardware. The test is "line
  ; 144 or later" and not "line 144" on purpose: the VBlank handler runs for
  ; several scanlines, so a poll for one exact line can be stepped over every
  ; single frame and never come back.
@wait:
  ldh a, [LY]
  cp 144
  jr c, @wait
  ldh a, [LCDC]
  and %01111111
  ldh [LCDC], a
  ret

rendering_on:
  ld a, [wLcdc]
  ldh [LCDC], a
  ret

; ---- scenes --------------------------------------------------------------

; Three parallel tables, one entry per screen: what to draw on entry, what to do
; each frame, and what to do inside the VBlank handler.

scene_enter_table:
  .word menu_enter, sprites_enter, scroll_enter, colors_enter, audio_enter, input_enter
scene_tick_table:
  .word menu_tick, sprites_tick, scroll_tick, colors_tick, audio_tick, input_tick
scene_vbl_table:
  .word scene_nothing, scene_nothing, scroll_vbl, colors_vbl, scene_nothing, scene_nothing

.assert scene_tick_table - scene_enter_table == 12, "six screens, two bytes each"
.assert scene_vbl_table - scene_tick_table == 12, "six screens, two bytes each"

scene_nothing:
  ret

scene_enter:
  ld a, [wScene]
  ld hl, scene_enter_table
  jp dispatch

scene_tick:
  ld a, [wScene]
  ld hl, scene_tick_table
  jp dispatch

scene_vbl:
  ld a, [wScene]
  ld hl, scene_vbl_table
  ; fall through

; a = index, hl = table of addresses. Jumps to the entry, leaving this routine's
; own return address on the stack, so the entry's `ret` goes back to the caller.
dispatch:
  add a
  ld c, a
  ld b, 0
  add hl, bc
  ld a, [hl+]
  ld h, [hl]
  ld l, a
  jp hl

change_scene:
  call rendering_off
  ld a, [wPending]
  ld [wScene], a
  ld a, SCENE_NONE
  ld [wPending], a
  ; Read the pad once and throw the result away. A button still held from the
  ; press that caused this change is now recorded as already held, so the
  ; incoming screen does not see it go down a second time and act on it: the A
  ; that enters the object screen must not also advance its arrangement.
  call read_joypad
  xor a
  ld [wJoyNew], a
  call scene_enter
  call rendering_on
  ret

; Everything every screen starts from, so a scene's enter routine only has to
; describe what is different about it.
reset_common:
  xor a
  ldh [STAT], a
  ldh [SCX], a
  ldh [SCY], a
  ld a, IE_VBLANK
  ldh [IE], a
  ld a, LCDCF_ON
  ld [wLcdc], a
  call clear_oam
  xor a                  ; tile $00 is the space
  call clear_map
  call default_palettes
  ret

; ---- the menu ------------------------------------------------------------

menu_enter:
  call reset_common
  ld hl, screen_menu
  call draw_script
  ; Which machine the core says this is. It is the first thing on the screen
  ; that the emulator, rather than the cartridge, decides.
  ld hl, str_dmg
  ld a, [wIsCgb]
  or a
  jr z, @write
  ld hl, str_cgb
@write:
  ld de, SCRN + 32*17 + 6
  ld b, 3
  call write_direct
  ret

str_dmg:
  .str "DMG"
str_cgb:
  .str "CGB"

menu_tick:
  ld a, [wJoyNew]
  ld b, a
  and BTN_UP
  jr z, @notup
  ld a, [wCursor]
  or a
  jr z, @notup
  dec a
  ld [wCursor], a
@notup:
  ld a, b
  and BTN_DOWN
  jr z, @notdown
  ld a, [wCursor]
  cp 4
  jr nc, @notdown
  inc a
  ld [wCursor], a
@notdown:
  ld a, b
  and BTN_A | BTN_START
  jr z, @noenter
  ld a, [wCursor]
  inc a
  ld [wPending], a
@noenter:
  ; The cursor and four objects idling on the rule, so that object rendering is
  ; visibly alive before anything has been chosen.
  ld hl, wOam
  ld a, [wCursor]
  add a
  add a
  add a
  add 6*8 + 16
  ld [hl+], a
  ld a, 2*8 + 8
  ld [hl+], a
  ld a, T_CURSOR
  ld [hl+], a
  xor a
  ld [hl+], a

  ld c, 0
@orb:
  ld a, c
  rrca
  rrca                   ; a quarter turn apart, so they bob out of step
  ld b, a
  ld a, [wFrame]
  add a
  add b
  call sine
  sub 128
  sra a
  sra a
  sra a
  sra a
  add 40 + 16
  ld [hl+], a
  ld a, c
  add a
  add a
  add a
  add a
  add a
  add 24 + 8
  ld [hl+], a
  ld a, T_ORB
  ld [hl+], a
  xor a
  ld [hl+], a
  inc c
  ld a, c
  cp 4
  jr nz, @orb
  ret

; ---- 1 objects -----------------------------------------------------------

sprites_enter:
  call reset_common
  ld hl, screen_sprites
  call draw_script
  xor a
  ld [wArrange], a
  ld [wBig], a
  ld [wOamRot], a
  ret

arrange_table:
  .word arrange_rings, arrange_grid, arrange_grid_rot, arrange_wave
arrange_names:
  .str "RINGS "
  .str "GRID  "
  .str "GRID+P"
  .str "WAVE  "
.assert arrange_table + 8 == arrange_names, "four arrangements, two bytes each"

sprites_tick:
  ld a, [wJoyNew]
  ld b, a
  and BTN_B
  jr z, @nob
  xor a
  ld [wPending], a
@nob:
  ld a, b
  and BTN_A
  jr z, @noa
  ld a, [wArrange]
  inc a
  and 3
  ld [wArrange], a
@noa:
  ld a, b
  and BTN_SELECT
  jr z, @nosel
  ld a, [wBig]
  xor 1
  ld [wBig], a
  ld a, [wLcdc]
  xor LCDCF_BIG
  ld [wLcdc], a
  ldh [LCDC], a
@nosel:
  ; The arrangement's name, in the six columns the header line leaves free.
  ld a, [wArrange]
  ld l, a
  ld h, 0
  add hl, hl
  ld d, h
  ld e, l
  add hl, hl
  add hl, de             ; index * 6
  ld de, arrange_names
  add hl, de
  ld de, SCRN + 32*1 + 14
  ld b, 6
  call queue_str

  call clear_oam
  ld a, [wArrange]
  ld hl, arrange_table
  jp dispatch

; Two rings turning against each other. Twenty objects each, so all forty are
; on screen and none of them is ever alone on its scanline.
arrange_rings:
  ld hl, wOam
  ld c, 0
@outer:
  ld a, c
  call spread            ; c * 13, so twenty of them nearly close the circle
  ld b, a
  ld a, [wFrame]
  add b
  push af
  call sine
  sub 128
  sra a
  add 72 + 16
  ld d, a
  pop af
  add 64                 ; a quarter turn on is the cosine
  call sine
  sub 128
  sra a
  add 80 + 8
  ld e, a
  ld a, d
  ld [hl+], a
  ld a, e
  ld [hl+], a
  ld a, T_ORB
  ld [hl+], a
  xor a
  ld [hl+], a
  inc c
  ld a, c
  cp 20
  jr nz, @outer

  ld c, 0
@inner:
  ld a, c
  call spread
  ld b, a
  ld a, [wFrame]
  cpl                    ; counting down instead of up turns it the other way
  add b
  push af
  call sine
  sub 128
  sra a
  sra a
  add 72 + 16
  ld d, a
  pop af
  add 64
  call sine
  sub 128
  sra a
  sra a
  add 80 + 8
  ld e, a
  ld a, d
  ld [hl+], a
  ld a, e
  ld [hl+], a
  ld a, T_GEM
  ld [hl+], a
  ; Object palette 1 on a colour Game Boy, OBP1 on a monochrome one: the same
  ; byte says both, so the two rings differ on either machine.
  ld a, %00010001
  ld [hl+], a
  inc c
  ld a, c
  cp 20
  jr nz, @inner
  ret

; a = n, returns n * 13.
spread:
  ld b, a
  add a
  add a
  add a
  add a
  sub b
  sub b
  sub b
  ret

; Two rows of twenty, every object in a row on the same scanline. The hardware
; draws ten objects per line and drops the rest, so exactly half of each row
; should be missing. If twenty render, the object evaluation is wrong.
arrange_grid:
  ld hl, wOam
  ld c, 0
@loop:
  ld a, c
  cp 20
  jr c, @row0
  ld d, 90 + 16
  sub 20
  jr @place
@row0:
  ld d, 60 + 16
@place:
  add a
  add a
  add a
  add 4 + 8
  ld e, a
  ld a, d
  ld [hl+], a
  ld a, e
  ld [hl+], a
  ld a, T_ORB
  ld [hl+], a
  xor a
  ld [hl+], a
  inc c
  ld a, c
  cp 40
  jr nz, @loop
  ret

; The same two rows, except that which object owns which OAM slot advances every
; sixteen frames. The ten that survive a line are the first ten in OAM order and
; not the ten furthest left, so the gaps have to march across the row.
arrange_grid_rot:
  ld a, [wFrame]
  and 15
  jr nz, @draw
  ld a, [wOamRot]
  inc a
  cp 20
  jr c, @keep
  xor a
@keep:
  ld [wOamRot], a
@draw:
  ld hl, wOam
  ld c, 0
@loop:
  ld a, c
  cp 20
  jr c, @row0
  ld d, 90 + 16
  sub 20
  jr @rotate
@row0:
  ld d, 60 + 16
@rotate:
  ld b, a
  ld a, [wOamRot]
  add b
  cp 20
  jr c, @place
  sub 20
@place:
  add a
  add a
  add a
  add 4 + 8
  ld e, a
  ld a, d
  ld [hl+], a
  ld a, e
  ld [hl+], a
  ld a, T_ORB
  ld [hl+], a
  xor a
  ld [hl+], a
  inc c
  ld a, c
  cp 40
  jr nz, @loop
  ret

; All forty across the full width of the screen, four pixels apart. Nowhere near
; ten of them share a line, so this is the arrangement that shows every object
; the cartridge has at once.
arrange_wave:
  ld hl, wOam
  ld c, 0
@loop:
  ld a, c
  add a
  add a
  add a
  ld e, a                ; the phase: eight steps per object
  ld a, [wFrame]
  add a
  add e
  call sine
  sub 128
  sra a
  add 72 + 16
  ld [hl+], a
  ld a, c
  add a
  add a
  add 8
  ld [hl+], a
  ld a, T_ORB
  ld [hl+], a
  xor a
  ld [hl+], a
  inc c
  ld a, c
  cp 40
  jr nz, @loop
  ret

; ---- 2 scroll split ------------------------------------------------------

scroll_enter:
  call reset_common
  call draw_playfield
  ld hl, screen_scroll
  call draw_script
  call scroll_colour
  xor a
  ld [wScrollX], a
  ld [wSpeed], a
  ld a, 1
  ld [wSplit], a
  call apply_split
  ret

; On a colour Game Boy the sky and the bricks get palettes of their own, so the
; split has something to separate other than two shades of grey. The status bar
; keeps palette 0, which is what makes a leak obvious: if the scroll reaches the
; bar, coloured sky appears behind the text.
scroll_colour:
  ld a, [wIsCgb]
  or a
  ret z
  ld hl, pal_sky
  ld a, 1
  call set_bg_pal
  ld hl, pal_brick
  ld a, 2
  call set_bg_pal
  ld d, 1
  ld b, SCROLL_BAR_ROWS
  ld c, 5
  call fill_attr_rows
  ld d, 2
  ld b, SCROLL_BAR_ROWS + 5
  ld c, 9
  call fill_attr_rows
  ret

; d = the attribute byte, b = the first row, c = how many rows. Fills the full
; 32 columns, so what scrolls in from the right is coloured too.
fill_attr_rows:
  ld a, [wIsCgb]
  or a
  ret z
  ld a, 1
  ldh [VBK], a
  ld l, b
  ld h, 0
  add hl, hl
  add hl, hl
  add hl, hl
  add hl, hl
  add hl, hl             ; row * 32
  push de
  ld de, SCRN
  add hl, de
  pop de
@row:
  ld b, 32
@col:
  ld a, d
  ld [hl+], a
  dec b
  jr nz, @col
  dec c
  jr nz, @row
  xor a
  ldh [VBK], a
  ret

; Sky above, bricks below, filling the whole 32-column map. The Game Boy has one
; background map and wraps it rather than a second one to walk into, so this is
; the whole playfield: what leaves the right edge comes back on the left.
draw_playfield:
  ld hl, SCRN + 32*5
  ld c, 5                ; the row, for the sky's own pattern
@sky_row:
  ld b, 32
  ld d, 0                ; the column
@sky_col:
  ld a, d
  add d
  add d                  ; column * 3
  add c
  add c
  add c
  add c
  add c                  ; + row * 5
  and 7
  jr z, @star
  cp 4
  jr z, @cloud
  xor a
  jr @put
@star:
  ld a, T_STAR
  jr @put
@cloud:
  ld a, T_CLOUD
@put:
  ld [hl+], a
  inc d
  dec b
  jr nz, @sky_col
  inc c
  ld a, c
  cp 9
  jr nz, @sky_row

  ; Nine rows of brick, the courses offset on alternate rows so that a scroll
  ; which tears or wraps early breaks a course visibly.
  ld c, 9
@brick_row:
  ld a, c
  and 1
  add T_BRICK_A
  ld d, a
  ld b, 32
@brick_col:
  ld a, d
  ld [hl+], a
  dec b
  jr nz, @brick_col
  dec c
  jr nz, @brick_row
  ret

scroll_tick:
  ld a, [wJoyNew]
  ld b, a
  and BTN_B
  jr z, @nob
  xor a
  ld [wPending], a
@nob:
  ld a, b
  and BTN_A
  jr z, @noa
  ld a, [wSplit]
  xor 1
  ld [wSplit], a
  call apply_split
@noa:
  ld a, b
  and BTN_RIGHT
  jr z, @noright
  ld a, [wSpeed]
  cp 4
  jr z, @noright
  inc a
  ld [wSpeed], a
@noright:
  ld a, b
  and BTN_LEFT
  jr z, @noleft
  ld a, [wSpeed]
  cp -4
  jr z, @noleft
  dec a
  ld [wSpeed], a
@noleft:
  ld a, [wSpeed]
  ld b, a
  ld a, [wScrollX]
  add b
  ld [wScrollX], a

  ; The speed, as a sign and a digit, in the columns the header leaves free.
  ld a, [wSpeed]
  bit 7, a
  jr z, @positive
  cpl
  inc a                  ; the magnitude of a negative speed
  ld b, a
  ld a, '-' - $20
  jr @sign
@positive:
  ld b, a
  ld a, '+' - $20
@sign:
  ld de, SCRN + 32*1 + 17
  call queue_tile
  inc de
  ld a, b
  add '0' - $20
  call queue_tile

  ; And whether the split is on, right after it.
  ld a, [wSplit]
  or a
  ld a, 'X' - $20
  jr z, @state
  ld a, '=' - $20
@state:
  ld de, SCRN + 32*2 + 18
  call queue_tile
  ret

apply_split:
  ld a, [wSplit]
  or a
  jr z, @off
  ld a, SPLIT_LINE
  ldh [LYC], a
  ld a, %01000000        ; interrupt when LY reaches LYC
  ldh [STAT], a
  ld a, IE_VBLANK | IE_STAT
  ldh [IE], a
  ret
@off:
  xor a
  ldh [STAT], a
  ld a, IE_VBLANK
  ldh [IE], a
  ret

; With the split on, the frame starts at scroll zero and the interrupt moves it
; once the bar is drawn. With the split off, the whole screen including the bar
; takes the scroll, which is what the split is being compared against.
scroll_vbl:
  ld a, [wSplit]
  or a
  ld a, 0
  jr nz, @set
  ld a, [wScrollX]
@set:
  ldh [SCX], a
  ret

; ---- 3 colours -----------------------------------------------------------

colors_enter:
  call reset_common
  ld hl, screen_colors
  call draw_script
  ld a, 24
  ld [wLevel], a
  ld a, 1
  ld [wPalDirty], a
  ld a, [wIsCgb]
  or a
  jr z, @mono
  call draw_swatches
  ret
@mono:
  call draw_shades
  ret

; Seven blocks, four tiles wide and three tall, each drawn on its own background
; palette. Palette 0 is the text, which is why the swatches start at 1.
draw_swatches:
  xor a
  ld [wBlockIndex], a
@block:
  ld a, [wBlockIndex]
  call swatch_origin     ; hl = the top left corner of this block
  push hl
  xor a
  ldh [VBK], a
  ld a, T_SWATCH
  call fill_block        ; the tiles themselves, in bank 0
  pop hl
  push hl
  ld a, 1
  ldh [VBK], a
  ld a, [wBlockIndex]
  inc a                  ; the palette each tile picks, in bank 1
  call fill_block
  pop hl
  xor a
  ldh [VBK], a
  ; The label, three rows under the block.
  ld de, 32*3
  add hl, de
  ld d, h
  ld e, l
  ld a, [wBlockIndex]
  add a
  ld l, a
  ld h, 0
  ld bc, swatch_labels
  add hl, bc
  ld b, 2
  call write_direct
  ld a, [wBlockIndex]
  inc a
  ld [wBlockIndex], a
  cp 7
  jr nz, @block
  ret

; a = which block, returns hl = the map address of its top left corner. Four
; across on rows 3 to 5, three across on rows 8 to 10.
swatch_origin:
  cp 4
  jr nc, @second
  ld b, a
  add a
  add a
  add b                  ; block * 5, so four wide with a column between
  ld l, a
  ld h, 0
  ld de, SCRN + 32*3
  add hl, de
  ret
@second:
  sub 4
  ld b, a
  add a
  add a
  add b
  ld l, a
  ld h, 0
  ld de, SCRN + 32*8 + 2
  add hl, de
  ret

; Four tiles across and three down, all the same byte, starting at hl.
fill_block:
  ld d, a
  ld b, 3
@row:
  push hl
  ld a, d
  ld [hl+], a
  ld [hl+], a
  ld [hl+], a
  ld [hl], a
  pop hl
  ld a, l
  add 32
  ld l, a
  ld a, h
  adc 0
  ld h, a
  dec b
  jr nz, @row
  ret

; A monochrome Game Boy has four shades and no palette memory to speak of, so
; the screen shows what it does have: the four shades side by side, under
; whichever BGP arrangement the level has selected.
draw_shades:
  xor a
  ld [wBlockIndex], a
@block:
  ld a, [wBlockIndex]
  ld b, a
  add a
  add a
  add b                  ; block * 5
  ld l, a
  ld h, 0
  ld de, SCRN + 32*4 + 2
  add hl, de
  push hl
  ld a, [wBlockIndex]
  or a
  jr z, @blank
  add T_SOLID1 - 1       ; shades 1, 2 and 3 have a tile each
  jr @fill
@blank:
  xor a                  ; shade 0 is the space, which is the background
@fill:
  call fill_block
  pop hl
  ld de, 32*3
  add hl, de
  ld d, h
  ld e, l
  ld a, [wBlockIndex]
  add a
  ld l, a
  ld h, 0
  ld bc, shade_labels
  add hl, bc
  ld b, 2
  call write_direct
  ld a, [wBlockIndex]
  inc a
  ld [wBlockIndex], a
  cp 4
  jr nz, @block
  ret

colors_tick:
  ld a, [wJoyNew]
  and BTN_B
  jr z, @nob
  xor a
  ld [wPending], a
@nob:
  ld a, [wJoyNew]
  and BTN_RIGHT
  jr z, @noright
  ld a, 1
  call level_up
@noright:
  ld a, [wJoyNew]
  and BTN_LEFT
  jr z, @noleft
  ld a, 1
  call level_down
@noleft:
  ld a, [wJoyNew]
  and BTN_UP
  jr z, @noup
  ld a, 8
  call level_up
@noup:
  ld a, [wJoyNew]
  and BTN_DOWN
  jr z, @nodown
  ld a, 8
  call level_down
@nodown:
  ld a, [wLevel]
  ld de, SCRN + 32*13 + 8
  call queue_hex8
  ret

level_up:
  ld b, a
  ld a, [wLevel]
  add b
  cp 32
  jr c, @store
  ld a, 31
@store:
  ld [wLevel], a
  ld a, 1
  ld [wPalDirty], a
  ret

level_down:
  ld b, a
  ld a, [wLevel]
  sub b
  jr nc, @store
  xor a
@store:
  ld [wLevel], a
  ld a, 1
  ld [wPalDirty], a
  ret

; Palette memory can only be written while the screen is not fetching from it,
; so the swatches are rebuilt here rather than in the tick.
colors_vbl:
  ld a, [wPalDirty]
  or a
  ret z
  xor a
  ld [wPalDirty], a
  ld a, [wIsCgb]
  or a
  jr z, @mono
  ; Seven combinations of the three channels at the chosen intensity: red,
  ; green, blue, then each pair, then all three. Every bit position in the
  ; fifteen-bit colour word gets exercised, which is the point.
  ;
  ; The counter is in memory rather than in a register because build_colour
  ; needs both halves of bc to take a colour apart.
  ld a, 1
  ld [wBlockIndex], a
@pal:
  ld a, [wBlockIndex]
  call build_colour      ; de = the colour this combination asks for
  ld a, [wBlockIndex]
  call write_swatch_pal
  ld a, [wBlockIndex]
  inc a
  ld [wBlockIndex], a
  cp 8
  jr nz, @pal
  ret
@mono:
  ; A monochrome Game Boy has no palette memory, so the level picks one of four
  ; BGP arrangements instead: normal, inverted, all black, all white.
  ld a, [wLevel]
  and 3
  ld e, a
  ld d, 0
  ld hl, bgp_modes
  add hl, de
  ld a, [hl]
  ldh [BGP], a
  ret

bgp_modes:
  .byte %11100100, %00011011, %11111111, %00000000

; a = which combination (1 to 7, as a red/green/blue bit mask), returns the
; fifteen-bit colour in de. The Game Boy Color stores it low byte first as
; %0BBBBBGG %GGGRRRRR.
build_colour:
  ld b, a
  ld a, [wLevel]
  ld c, a
  ld d, 0
  ld e, 0
  bit 0, b
  jr z, @green
  ld a, e
  or c
  ld e, a                ; red is the low five bits
@green:
  bit 1, b
  jr z, @blue
  ; green straddles the byte boundary: three bits at the top of the low byte,
  ; two at the bottom of the high one
  ld a, c
  and %00000111
  rrca
  rrca
  rrca                   ; into bits 5 to 7
  or e
  ld e, a
  ld a, c
  and %00011000
  rrca
  rrca
  rrca
  or d
  ld d, a
@blue:
  bit 2, b
  jr z, @done
  ld a, c
  add a
  add a                  ; blue starts at bit 10, which is bit 2 of the high byte
  or d
  ld d, a
@done:
  ret

; Palette a gets colour 0 white, colour 1 the colour in de, colour 2 a dimmed
; version of it, colour 3 black. The swatch tile draws its middle in colour 1
; and its border in colour 3.
write_swatch_pal:
  add a
  add a
  add a
  or $80                 ; auto-increment through the eight bytes
  ldh [BCPS], a
  ld a, $ff
  ldh [BCPD], a
  ld a, $7f
  ldh [BCPD], a
  ld a, e
  ldh [BCPD], a
  ld a, d
  ldh [BCPD], a
  ; The dim version: every channel halved, which a single shift of the whole
  ; word does not do, so it is done channel by channel.
  ld a, e
  and %00011111
  srl a
  ld b, a
  ld a, e
  and %11100000
  srl a
  and %11100000
  or b
  ldh [BCPD], a
  ld a, d
  srl a
  and %00111111
  ldh [BCPD], a
  xor a
  ldh [BCPD], a
  ldh [BCPD], a
  ret

; ---- 4 audio -------------------------------------------------------------

audio_enter:
  call reset_common
  ld hl, screen_audio
  call draw_script
  call start_audio
  xor a
  ld [wChanSel], a
  ld [wMute], a
  ld [wStep], a
  ld [wTick], a
  call apply_mute
  ret

; Every branch here reloads the button word rather than keeping it in a
; register, because apply_mute and bit_mask both write to b.
audio_tick:
  ld a, [wJoyNew]
  and BTN_B
  jr z, @nob
  call silence_audio
  xor a
  ld [wPending], a
@nob:
  ld a, [wJoyNew]
  and BTN_UP
  jr z, @noup
  ld a, [wChanSel]
  or a
  jr z, @noup
  dec a
  ld [wChanSel], a
@noup:
  ld a, [wJoyNew]
  and BTN_DOWN
  jr z, @nodown
  ld a, [wChanSel]
  cp 3
  jr nc, @nodown
  inc a
  ld [wChanSel], a
@nodown:
  ld a, [wJoyNew]
  and BTN_A
  jr z, @noa
  ld a, [wChanSel]
  call bit_mask          ; a = 1 << channel
  ld b, a
  ld a, [wMute]
  xor b
  ld [wMute], a
  call apply_mute
@noa:
  ld a, [wJoyNew]
  and BTN_START
  jr z, @nostart
  xor a
  ld [wStep], a
  ld [wTick], a
@nostart:
  call music_advance
  call audio_labels
  ret

; a = n, returns 1 << n for n in 0 to 7.
bit_mask:
  ld b, a
  ld a, 1
  inc b
@shift:
  dec b
  ret z
  add a
  jr @shift

; The cursor, and each channel's state word.
audio_labels:
  ld c, 0
@row:
  ; The cursor sits in column 1 of the channel's row: rows 4, 6, 8, 10.
  ld a, c
  add a
  add 4
  call row_address       ; de = SCRN + row * 32
  inc de
  ld a, [wChanSel]
  cp c
  ld a, 0
  jr nz, @blank
  ld a, T_CURSOR
@blank:
  call queue_tile

  ; ...and its state, in the four columns the header labelled STATE.
  ld a, c
  add a
  add 4
  call row_address
  ld hl, 15
  add hl, de
  ld d, h
  ld e, l
  ld a, c
  call bit_mask
  ld b, a
  ld a, [wMute]
  and b
  ld hl, str_on
  jr z, @word
  ld hl, str_mute
@word:
  ld b, 4
  call queue_str
  inc c
  ld a, c
  cp 4
  jr nz, @row
  ret

; a = row, returns de = the map address of its first column.
row_address:
  ld l, a
  ld h, 0
  add hl, hl
  add hl, hl
  add hl, hl
  add hl, hl
  add hl, hl             ; row * 32
  ld de, SCRN
  add hl, de
  ld d, h
  ld e, l
  ret

; Sixteen steps, one every eight frames.
music_advance:
  ld a, [wTick]
  inc a
  cp 8
  jr c, @store
  xor a
  ld [wTick], a
  ld a, [wStep]
  inc a
  and 15
  ld [wStep], a
  jp music_step
@store:
  ld [wTick], a
  ret

music_step:
  ld a, [wStep]
  ld e, a
  ld d, 0

  ld hl, music_ch1
  add hl, de
  ld a, [hl]
  cp REST
  jr z, @two
  ld b, 0
  call trigger_pulse
@two:
  ld hl, music_ch2
  add hl, de
  ld a, [hl]
  cp REST
  jr z, @three
  ld b, 1
  call trigger_pulse
@three:
  ld hl, music_ch3
  add hl, de
  ld a, [hl]
  cp REST
  jr z, @four
  call trigger_wave
@four:
  ld hl, music_ch4
  add hl, de
  ld a, [hl]
  cp REST
  ret z
  jp trigger_noise

; a = note index, b = 0 for pulse A or 1 for pulse B. A muted channel is not
; retriggered at all, on top of having had its digital-to-analogue converter
; switched off, so nothing about it is left half on.
trigger_pulse:
  ld c, a                ; the note, while the mute bit is worked out
  ld a, %00000001
  bit 0, b
  jr z, @mask
  ld a, %00000010
@mask:
  ld e, a
  ld a, [wMute]
  and e
  ret nz
  ld a, c
  call note_period       ; de = the eleven-bit period
  ld a, b
  or a
  jr nz, @second
  ld a, e
  ldh [NR13], a
  ld a, d
  or %10000000           ; trigger
  ldh [NR14], a
  ret
@second:
  ld a, e
  ldh [NR23], a
  ld a, d
  or %10000000
  ldh [NR24], a
  ret

trigger_wave:
  ld c, a
  ld a, [wMute]
  and %00000100
  ret nz
  ld a, c
  call note_period
  ld a, e
  ldh [NR33], a
  ld a, d
  or %10000000
  ldh [NR34], a
  ret

; a is an NR43 value here, not a note: the noise channel is programmed with a
; shift and a divisor rather than a period.
trigger_noise:
  ld b, a
  ld a, [wMute]
  and %00001000
  ret nz
  ld a, b
  ldh [NR43], a
  ld a, %10000000
  ldh [NR44], a
  ret

; a = note index, returns de = the period from note_table.
note_period:
  ld l, a
  ld h, 0
  add hl, hl
  ld de, note_table
  add hl, de
  ld a, [hl+]
  ld e, a
  ld a, [hl]
  ld d, a
  ret

; The envelope registers are also the digital-to-analogue converter switches: a
; channel whose envelope byte is zero is off, not merely quiet.
apply_mute:
  ld a, [wMute]
  ld b, a
  bit 0, b
  ld a, $f3
  jr z, @one
  xor a
@one:
  ldh [NR12], a
  bit 1, b
  ld a, $f3
  jr z, @two
  xor a
@two:
  ldh [NR22], a
  bit 2, b
  ld a, $80
  jr z, @three
  xor a
@three:
  ldh [NR30], a
  bit 3, b
  ld a, $f2
  jr z, @four
  xor a
@four:
  ldh [NR42], a
  ret

start_audio:
  ld a, %10000000        ; power on before anything else, or writes are ignored
  ldh [NR52], a
  ld a, %01110111        ; both sides at full volume
  ldh [NR50], a
  ld a, %11111111        ; every channel to both sides
  ldh [NR51], a

  xor a
  ldh [NR10], a          ; no sweep on pulse A
  ld a, %10000000        ; half duty
  ldh [NR11], a
  ld a, %01000000        ; quarter duty on pulse B, so the two are told apart
  ldh [NR21], a
  xor a
  ldh [NR31], a
  ld a, %00100000        ; wave at full output
  ldh [NR32], a
  xor a
  ldh [NR41], a

  ; The wave channel's own sample memory. It has to be written with the channel
  ; switched off, which is what the zero into NR30 is for.
  xor a
  ldh [NR30], a
  ld hl, wave_pattern
  ld c, <WAVERAM
  ld b, 16
@wave:
  ld a, [hl+]
  ldh [c], a
  inc c
  dec b
  jr nz, @wave
  ld a, %10000000
  ldh [NR30], a
  ret

; Everything off, and the sound hardware itself powered down, which is the only
; way to be sure a channel is not still holding a level.
silence_audio:
  xor a
  ldh [NR12], a
  ldh [NR22], a
  ldh [NR30], a
  ldh [NR42], a
  ldh [NR52], a
  ret

; ---- 5 input -------------------------------------------------------------

input_enter:
  call reset_common
  ld hl, screen_input
  call draw_script
  ret

input_tick:
  ld a, [wJoyNew]
  and BTN_B
  jr z, @go
  xor a
  ld [wPending], a
@go:
  ; One indicator per button, in the order the joypad byte reports them.
  ld a, [wJoy]
  ld b, a
  ld c, 0
@key:
  ld l, c
  ld h, 0
  add hl, hl
  ld de, key_cells
  add hl, de
  ld a, [hl+]
  ld e, a
  ld a, [hl]
  ld d, a
  srl b                  ; the low bit is the button this cell belongs to
  ld a, T_KEY_OFF
  jr nc, @put
  ld a, T_KEY_ON
@put:
  call queue_tile
  inc c
  ld a, c
  cp 8
  jr nz, @key

  ; The two halves of the joypad register, exactly as they were read, so that a
  ; core which never lets the lines settle shows it here.
  ld a, [wRawDpad]
  ld de, SCRN + 32*13 + 11
  call queue_hex8
  ld a, [wRawBtn]
  ld de, SCRN + 32*14 + 11
  call queue_hex8
  ld a, [wFrame + 1]
  ld de, SCRN + 32*15 + 9
  call queue_hex8
  ld a, [wFrame]
  ld de, SCRN + 32*15 + 11
  call queue_hex8
  ret

; ---- joypad --------------------------------------------------------------

; Select one half of the matrix, read it several times so the lines have settled
; before the value is believed, then invert it: the hardware reports a pressed
; button as a zero, and every other line of this cartridge would rather it did
; not.
read_joypad:
  ld a, %00100000        ; directions
  ldh [P1], a
  ldh a, [P1]
  ldh a, [P1]
  ldh a, [P1]
  ld [wRawDpad], a
  cpl
  and $0f
  swap a
  ld b, a

  ld a, %00010000        ; buttons
  ldh [P1], a
  ldh a, [P1]
  ldh a, [P1]
  ldh a, [P1]
  ldh a, [P1]
  ldh a, [P1]
  ldh a, [P1]
  ld [wRawBtn], a
  cpl
  and $0f
  or b
  ld c, a

  ld a, %00110000        ; neither half selected
  ldh [P1], a

  ld a, [wJoy]
  ld [wJoyPrev], a
  cpl
  and c
  ld [wJoyNew], a
  ld a, c
  ld [wJoy], a
  ret

; ---- background writes ---------------------------------------------------

; Runs of tiles, each a destination address, a length, then that many tiles.
; A zero destination ends it. Only ever called with the screen off.
draw_script:
  ld a, [hl+]
  ld e, a
  ld a, [hl+]
  ld d, a
  or e
  ret z
  ld a, [hl+]
  ld b, a
@copy:
  ld a, [hl+]
  ld [de], a
  inc de
  dec b
  jr nz, @copy
  jr draw_script

; de = destination, hl = source, b = count. Straight into video memory, so the
; screen has to be off.
write_direct:
  ld a, [hl+]
  ld [de], a
  inc de
  dec b
  jr nz, write_direct
  ret

; de = address, a = tile. Leaves the write for the VBlank handler to make.
queue_tile:
  push hl
  push bc
  ld b, a
  ld a, [wQueueLen]
  cp QUEUE_MAX
  jr nc, @full
  ld c, a
  inc a
  ld [wQueueLen], a
  ld a, c
  add a
  add c                  ; three bytes per entry
  ld c, a
  ld hl, wQueue
  ld a, l
  add c
  ld l, a
  ld a, h
  adc 0
  ld h, a
  ld [hl], e
  inc hl
  ld [hl], d
  inc hl
  ld [hl], b
@full:
  pop bc
  pop hl
  ret

; de = address, hl = source, b = count.
queue_str:
  ld a, [hl+]
  call queue_tile
  inc de
  dec b
  jr nz, queue_str
  ret

; de = address, a = value. Two tiles, high nibble first.
queue_hex8:
  push af
  swap a
  and $0f
  call hex_digit
  call queue_tile
  inc de
  pop af
  and $0f
  call hex_digit
  jp queue_tile

hex_digit:
  cp 10
  jr c, @digit
  add 'A' - $20 - 10
  ret
@digit:
  add '0' - $20
  ret

drain_queue:
  ld a, [wQueueLen]
  or a
  ret z
  ld b, a
  ld hl, wQueue
@entry:
  ld a, [hl+]
  ld e, a
  ld a, [hl+]
  ld d, a
  ld a, [hl+]
  ld [de], a
  dec b
  jr nz, @entry
  xor a
  ld [wQueueLen], a
  ret

; ---- memory --------------------------------------------------------------

clear_wram:
  ld hl, $c000
  ld bc, $2000
@loop:
  xor a
  ld [hl+], a
  dec bc
  ld a, b
  or c
  jr nz, @loop
  ret

; a = tile. Fills the background map, and on a colour Game Boy also clears the
; attribute map behind it, which starts as whatever was in the chip.
clear_map:
  ld d, a
  xor a
  ldh [VBK], a
  ld hl, SCRN
  ld bc, $0400
@loop:
  ld a, d
  ld [hl+], a
  dec bc
  ld a, b
  or c
  jr nz, @loop
  ld a, [wIsCgb]
  or a
  ret z
  ld a, 1
  ldh [VBK], a
  ld hl, SCRN
  ld bc, $0400
@attr:
  xor a
  ld [hl+], a
  dec bc
  ld a, b
  or c
  jr nz, @attr
  xor a
  ldh [VBK], a
  ret

clear_oam:
  ld hl, wOam
  ld b, 160
@loop:
  xor a
  ld [hl+], a
  dec b
  jr nz, @loop
  ret

load_tiles:
  xor a
  ldh [VBK], a
  ld hl, tiles_start
  ld de, VRAM
  ld bc, tiles_end - tiles_start
@loop:
  ld a, [hl+]
  ld [de], a
  inc de
  dec bc
  ld a, b
  or c
  jr nz, @loop
  ret

; The object transfer routine has to run from high RAM: while the controller is
; moving the buffer the CPU can see nothing else.
install_dma:
  ld hl, dma_source
  ld de, dma_start
  ld b, dma_source_end - dma_source
@loop:
  ld a, [hl+]
  ld [de], a
  inc de
  dec b
  jr nz, @loop
  ret

dma_source:
  ldh [DMA], a
  ld a, 40               ; the transfer takes 160 machine cycles; this is 160
@wait:
  dec a
  jr nz, @wait
  ret
dma_source_end:

; ---- palettes ------------------------------------------------------------

default_palettes:
  ld a, [wIsCgb]
  or a
  jr nz, @colour
  ld a, %11100100
  ldh [BGP], a
  ldh [OBP0], a
  ld a, %11010000
  ldh [OBP1], a
  ret
@colour:
  ld hl, pal_text
  xor a
  call set_bg_pal
  ld hl, pal_obj_orb
  xor a
  call set_obj_pal
  ld hl, pal_obj_gem
  ld a, 1
  call set_obj_pal
  ret

; a = palette number, hl = four colours. Auto-increment walks the eight bytes.
set_bg_pal:
  add a
  add a
  add a
  or $80
  ldh [BCPS], a
  ld b, 8
@byte:
  ld a, [hl+]
  ldh [BCPD], a
  dec b
  jr nz, @byte
  ret

set_obj_pal:
  add a
  add a
  add a
  or $80
  ldh [OCPS], a
  ld b, 8
@byte:
  ld a, [hl+]
  ldh [OCPD], a
  dec b
  jr nz, @byte
  ret

; ---- arithmetic ----------------------------------------------------------

; a = angle, returns a = the sine of it, centred on 128. Adding 64 to the angle
; before calling makes it a cosine; the table is exactly 256 entries so the
; addition is allowed to wrap.
sine:
  push hl
  push de
  ld e, a
  ld d, 0
  ld hl, sine_table
  add hl, de
  ld a, [hl]
  pop de
  pop hl
  ret

.include "data.s"
.include "tables.s"
.include "tiles.s"

; ---- layout checks -------------------------------------------------------
; Evaluated once every label is final, which is what makes them worth writing.

.assert tiles_end <= $7fff, "the cartridge has outgrown its 32 KB"
.assert dma_source_end - dma_source <= 16, "the DMA routine no longer fits in high RAM"
.assert (wOam & $ff) == 0, "the object buffer must start on a page boundary"
.assert wQueue >= $c100, "the queue overlaps the object buffer"
