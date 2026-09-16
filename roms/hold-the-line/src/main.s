; SPDX-License-Identifier: CC0-1.0
; HOLD THE LINE. Dedicated to the public domain; see LICENSE.
;
; An element-drafting tower defence for the Game Boy Color, for one player or
; for two over a link cable. See DESIGN.md beside this file.
;
; Assembled by gb-asm, in this repository. Nothing outside this repository is
; needed to turn this file into the .gbc beside it.

; ---- hardware --------------------------------------------------------------

P1    = $ff00        ; joypad: write a selector, read the four lines back
DIV   = $ff04
IF    = $ff0f

SB    = $ff01        ; the link cable: data, then control
SC    = $ff02

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

; Where the object transfer routine is copied to and called from.
dma_start = $ff80

; LCDC: on, window map at $9C00, window off, tile data at $8000, BG map at
; $9800, 8x8 objects, objects on, background on.
LCDCF_ON = %10010011

; The joypad byte this cartridge builds: one bit per button, pressed = 1.
;
;   bit 0 A   bit 1 B   bit 2 select   bit 3 start
;   bit 4 right   bit 5 left   bit 6 up   bit 7 down
;
; `bit` wants a literal number rather than a name, so the tests below say the
; number and the comment beside them says the button.

; Object attribute bits.
OAM_XFLIP = %00100000
OAM_YFLIP = %01000000

; ---- the board -------------------------------------------------------------
; Ten cells by eight, each 16 by 16 pixels, under a status bar two tiles tall.
; 160 by 16 plus 160 by 128 is 160 by 144 exactly, so nothing is centred and
; there is no margin anywhere to get wrong.

; The screen is 160 by 144, which is 20 tiles by 18. A status bar across the top
; costs two of those rows and caps the board at sixteen, and sixteen turned out
; to be one row short of the map that was wanted.
;
; So the interface moved to a panel down the RIGHT instead. That gives the board
; all eighteen rows to use, and it gives the element draft somewhere to live
; later, which a two-row strip never would have. The cost is width: the panel
; needs six columns, so the board can be at most fourteen.
GRID_W = 14
GRID_H = 17
GRID_CELLS = GRID_W * GRID_H
FIELD_ROW = 0                    ; the map row the field starts on
HUD_COL = 14                     ; the first map column the panel owns
HUD_W = 20 - HUD_COL

; A cell is one tile, and the map is 32 tiles wide.
ROW_STRIDE = 32

; Long enough for a route that fills the board, and a hard stop for a malformed
; map: the walk gives up here rather than writing past the list.
PATH_MAX = 176

; How many creeps may be on the board at once. Sixteen 8x8 objects plus the four
; the cursor uses is twenty of the forty the hardware has, which leaves room for
; projectiles and effects later without revisiting this number.
CREEP_MAX = 16

; A creep adds this to its position each frame, out of the 256 that make up one
; cell. A cell is eight pixels, so 16 is half a pixel a frame and a creep takes
; about 36 seconds to cross the whole 134 cell route.
;
; Select cycles through creep_speeds while the game runs and the status bar shows
; which one is live, because the right number for this is a thing to find by
; watching it rather than by reasoning about it.
CREEP_SPEED = 16
CREEP_SPEEDS = 6

; The trickle that stands in for the wave table until there is one.
SPAWN_GAP = 40
WAVE_GAP = 150           ; the breather between waves, where the draft will go
WAVE_SIZE = 8

START_LIVES = 20

; Where each two-digit field sits in the side panel, and the tile the digit zero
; is. A tile index is its character minus $20, and '0' is $30.
LIVES_AT = SCRN + 1 * 32 + HUD_COL + 3
WAVE_AT  = SCRN + 4 * 32 + HUD_COL + 3
SPEED_AT = SCRN + 7 * 32 + HUD_COL + 3
TILE_DIGIT0 = $10

CELL_GROUND = 0
CELL_PATH   = 1
CELL_SPAWN  = 2
CELL_EXIT   = 3
CELL_KINDS  = 4

; Background palettes.
PAL_UI     = 0
PAL_GROUND = 1
PAL_PATH   = 2
PAL_SPAWN  = 3
PAL_EXIT   = 4

; Object palettes.
OPAL_CURSOR = 0
OPAL_CREEP  = 1

; Tile numbers. The font is $00 to $3F, so the graphics start at $40. Each of
; these is tied to its picture by an assertion at the end of this file, because
; a tile number that has quietly drifted from its art is not a thing you find by
; reading either one.
TILE_BLANK   = $00               ; space, the first character of the font
TILE_GROUND  = $40
TILE_PATH    = $41
TILE_CURSOR  = $42
TILE_CREEP   = $43

; ---- the cartridge ---------------------------------------------------------
; No mapper and 32 KB, which is the widest-compatibility Game Boy cartridge
; there is: it is the shape a Game Boy addresses directly, with no bank
; switching hardware between the CPU and the ROM at all.
;
; cgb=on marks it as using colour where there is colour while still running on a
; monochrome Game Boy. That is a promise the game has to keep, which is why an
; element is a tower silhouette as well as a palette.

.gb title="HOLD THE LINE" cgb=on sgb=off mbc=none rom=32 version=0

; ---- work RAM --------------------------------------------------------------
; Addresses only. A cartridge cannot ship RAM contents, so nothing here emits a
; byte; the reset routine clears the lot.

.ram $c000
; The object attribute buffer. It has to start on a $100 boundary because the
; DMA controller takes only the high byte of the source address.
wOam:          .res 160

.ram $c100
; The board, decoded from the picture in data.s: one cell kind per cell.
wCells:        .res GRID_CELLS
; The route, derived from the board. wPath is cell indices, from the spawn to
; the exit; the other three are the same waypoints pre-chewed into the forms
; everything downstream actually wants, worked out during the walk when the
; column and row are already in hand. Drawing a creep never turns a cell index
; back into a column, so nothing in the frame loop ever divides by ten.
; There is deliberately no array of cell INDICES here any more. A 20 by 16 board
; has 320 cells, so an index no longer fits in a byte, and everything that used
; one wanted a column and a row in the end anyway.
wPathCol:      .res PATH_MAX
wPathRow:      .res PATH_MAX
wPathDir:      .res PATH_MAX    ; 0 right, 1 down, 2 left, 3 up, to the next one

.ram $c500
wVBlankDone:   .res 1    ; set by the VBlank handler, cleared by the main loop
wFrame:        .res 2    ; frames since reset, low byte first
wJoy:          .res 1    ; buttons held now
wJoyNew:       .res 1    ; buttons that went down this frame
wIsCgb:        .res 1
wLcdc:         .res 1    ; what rendering_on turns the screen back on with

wPathLen:      .res 1    ; waypoints in wPath
wCurX:         .res 1    ; build cursor, in cells
wCurY:         .res 1

; The path walk's own state. It runs once per map, so it is written for clarity
; rather than for registers.
wWalkX:        .res 1
wWalkY:        .res 1
wWalkPrevX:    .res 1    ; where it came from, so it cannot turn round
wWalkPrevY:    .res 1
wWalkDir:      .res 1    ; which of the four neighbours is under test

wLives:        .res 1
wWave:         .res 1
wSpeedSel:     .res 1    ; index into creep_speeds
wSpawnLeft:    .res 1    ; creeps still to come in this wave
wSpawnTimer:   .res 1    ; frames until the next one

; Scratch for drawing one creep. Same reasoning as the walk's: this runs sixteen
; times a frame at most, so it is written to be read rather than to save a push.
wDrawX:        .res 1
wDrawY:        .res 1
wDrawWp:       .res 1    ; the waypoint it has reached
wDrawOff:      .res 1    ; and how far past it, in pixels

; One array per field rather than one struct per creep: indexing is a single add
; of the slot number, which on this CPU is the difference between a lookup and a
; multiply.
.ram $c600
wCreepAlive:   .res CREEP_MAX
wCreepIdx:     .res CREEP_MAX    ; waypoint reached
wCreepSub:     .res CREEP_MAX    ; 0-255 across the sixteen pixels to the next
wCreepSpeed:   .res CREEP_MAX

; ---- interrupt vectors -----------------------------------------------------

.org $0040
  jp vblank_isr

.org $0048
  reti                   ; LCD status, unused so far

.org $0050
  reti                   ; timer, unused

.org $0058
  reti                   ; serial: the link cable will land here

.org $0060
  reti                   ; joypad, unused: this cartridge polls instead

; ---- header ----------------------------------------------------------------

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

; ---- reset -----------------------------------------------------------------

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

  ld a, LCDCF_ON
  ld [wLcdc], a

  call load_palettes
  call start_map

  ld a, IE_VBLANK
  ldh [IE], a
  xor a
  ldh [IF], a
  ei
  call rendering_on

main:
  call wait_frame
  call read_joypad
  call move_cursor
  call cycle_speed
  call wave_tick
  call update_creeps
  call build_oam
  jr main

; ---- frame -----------------------------------------------------------------

; Block until the VBlank handler has run. Everything the game does happens
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
  call draw_status_numbers
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

; ---- screen on and off -----------------------------------------------------

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

; ---- starting a map --------------------------------------------------------

; Decode the picture, draw it, derive the route, and put the cursor somewhere
; sensible. Runs with the screen off: it writes most of the tile map.
start_map:
  call lcd_off
  ld a, TILE_BLANK
  call clear_map
  call clear_oam
  ld de, map_1
  call decode_map
  call draw_field
  call draw_hud
  call build_path
  call clear_creeps
  ld a, START_LIVES
  ld [wLives], a
  ld a, 1
  ld [wWave], a
  ld a, 2                ; creep_speeds[2] is CREEP_SPEED
  ld [wSpeedSel], a
  ld a, WAVE_SIZE
  ld [wSpawnLeft], a
  ld a, SPAWN_GAP
  ld [wSpawnTimer], a
  ; The cursor starts on the top-left cell, which map 1 makes the spawn. That is
  ; fine: the cursor is allowed to sit on the path, it just cannot build there.
  xor a
  ld [wCurX], a
  ld [wCurY], a
  ret

; de -> a map picture, GRID_CELLS characters of it. Writes one cell kind per
; cell into wCells.
;
; .str already emitted each character as its code minus $20, which is exactly an
; index into cell_kind, so the picture needs no unpacking of any kind.
decode_map:
  ld hl, wCells
  ld bc, GRID_CELLS
@cell:
  ld a, [de]
  inc de
  push bc
  push de
  ld e, a
  ld d, 0
  push hl
  ld hl, cell_kind
  add hl, de
  ld a, [hl]
  pop hl
  pop de
  pop bc
  ld [hl+], a
  dec bc
  ld a, b
  or c
  jr nz, @cell
  ret

; Draw wCells onto the background map. Screen off.
draw_field:
  ld de, wCells
  ld hl, SCRN + FIELD_ROW * 32
  ld c, GRID_H
@row:
  push hl
  ld b, GRID_W
@col:
  ld a, [de]
  inc de
  push de
  call draw_cell
  pop de
  inc hl
  dec b
  jr nz, @col
  pop hl
  ; Down one cell row, which is two map rows.
  ld a, l
  add ROW_STRIDE
  ld l, a
  jr nc, @carried
  inc h
@carried:
  dec c
  jr nz, @row
  ret

; a = cell kind, hl = the map address of the cell. Writes the tile, and on a
; colour Game Boy the attribute behind it. Preserves hl, bc and de.
draw_cell:
  push bc
  push de
  ld c, a
  ld b, 0
  push hl
  ld hl, cell_tiles
  add hl, bc
  ld a, [hl]
  ld d, a
  ld hl, cell_palette
  add hl, bc
  ld a, [hl]
  ld e, a
  pop hl
  xor a
  ldh [VBK], a
  ld [hl], d
  ld a, [wIsCgb]
  or a
  jr z, @done
  ld a, 1
  ldh [VBK], a
  ld [hl], e
  xor a
  ldh [VBK], a
@done:
  pop de
  pop bc
  ret

; c = column, b = row. Returns hl pointing at that cell in wCells. Preserves bc.
;
; This exists because 320 cells no longer fit a byte, so row_base is a table of
; WORDS and the arithmetic has to be sixteen bit the whole way.
cell_ptr:
  push de
  ld e, b
  ld d, 0
  ld hl, row_base
  add hl, de
  add hl, de
  ld a, [hl+]
  ld h, [hl]
  ld l, a                ; row * GRID_W
  ld de, wCells
  add hl, de
  ld e, c
  ld d, 0
  add hl, de
  pop de
  ret

; hl -> the cell the walk is currently standing on.
walk_ptr:
  push bc
  ld a, [wWalkX]
  ld c, a
  ld a, [wWalkY]
  ld b, a
  call cell_ptr
  pop bc
  ret

; ---- the route -------------------------------------------------------------

; Walk the board from the spawn and record the route.
;
; The picture in data.s is the only place the route is written down. This derives
; the order from it, so there is no second copy to fall out of agreement with the
; first. Each waypoint is stored as a column and a row rather than a cell index,
; both because 320 cells do not fit in a byte and because a column and a row are
; what drawing a creep actually wants.
build_path:
  xor a
  ld [wPathLen], a
  ld hl, wCells
  ld b, 0                ; row
@row:
  ld c, 0                ; column
@col:
  ld a, [hl+]
  cp CELL_SPAWN
  jr z, @found
  inc c
  ld a, c
  cp GRID_W
  jr nz, @col
  inc b
  ld a, b
  cp GRID_H
  jr nz, @row
  ; No spawn on this map. Leave the route empty rather than walking off the
  ; board; checkmap.py is what stops this reaching a cartridge.
  ret

@found:
  ld a, b
  ld [wWalkY], a
  ld a, c
  ld [wWalkX], a
  ; $FF is not a column or a row any map can have, so the first step has nowhere
  ; it is forbidden to go.
  ld a, $ff
  ld [wWalkPrevX], a
  ld [wWalkPrevY], a
  ld b, 0                ; waypoints written so far

@step:
  ld e, b
  ld d, 0
  ld hl, wPathCol
  add hl, de
  ld a, [wWalkX]
  ld [hl], a
  ld hl, wPathRow
  add hl, de
  ld a, [wWalkY]
  ld [hl], a
  inc b

  ; Stop at the exit.
  call walk_ptr
  ld a, [hl]
  cp CELL_EXIT
  jr z, @done
  ; A malformed map must not write past the end of the list.
  ld a, b
  cp PATH_MAX
  jr nc, @done
  ; walk_next needs every register it can get, and this loop is holding the
  ; count in b. Popping does not touch the flags, so the "did it move" answer
  ; survives being handed the registers back.
  push bc
  call walk_next
  pop bc
  jr z, @done
  ; The direction it just took is the direction away from the waypoint written
  ; last, which is the one at b - 1. The last waypoint never gets one, because
  ; there is nowhere to go from the exit.
  ld a, b
  dec a
  ld e, a
  ld d, 0
  ld hl, wPathDir
  add hl, de
  ld a, [wWalkDir]
  ld [hl], a
  jr @step

@done:
  ld a, b
  ld [wPathLen], a
  ret

; Step the walk to the one neighbour that is walkable and is not where it came
; from. Returns nz if it moved, z if it is stuck. Clobbers a, bc, de, hl.
walk_next:
  xor a
  ld [wWalkDir], a
@try:
  ld a, [wWalkDir]
  add a
  ld e, a
  ld d, 0
  ld hl, walk_dirs
  add hl, de
  ; A column of $FF is a step left and a column of GRID_W is a step off the
  ; right edge. One unsigned comparison against the width rejects both, which is
  ; why the deltas are stored as bytes and not as a sign and a magnitude.
  ld a, [wWalkX]
  add [hl]
  cp GRID_W
  jr nc, @next
  ld c, a                ; the neighbour's column
  inc hl
  ld a, [wWalkY]
  add [hl]
  cp GRID_H
  jr nc, @next
  ld b, a                ; the neighbour's row
  ; Not where it came from. Both halves have to match for it to be the same
  ; cell, which is why a column that differs falls straight through to the
  ; walkable test rather than rejecting.
  ld a, [wWalkPrevX]
  cp c
  jr nz, @walkable
  ld a, [wWalkPrevY]
  cp b
  jr z, @next
@walkable:
  call cell_ptr
  ld a, [hl]
  ld e, a
  ld d, 0
  ld hl, cell_walkable
  add hl, de
  ld a, [hl]
  or a
  jr z, @next
  ; Take it. Where the walk stands now becomes where it came from, which is what
  ; stops it turning round at the next step.
  ld a, [wWalkX]
  ld [wWalkPrevX], a
  ld a, [wWalkY]
  ld [wWalkPrevY], a
  ld a, c
  ld [wWalkX], a
  ld a, b
  ld [wWalkY], a
  ld a, 1
  or a
  ret
@next:
  ld hl, wWalkDir
  inc [hl]
  ld a, [hl]
  cp 4
  jr nz, @try
  xor a
  ret

; The four neighbours, as (column, row) steps: right, down, left, up.
walk_dirs:
  .byte 1, 0
  .byte 0, 1
  .byte $ff, 0
  .byte 0, $ff

; The four tiles each cell kind is drawn with, in the order blit_quad wants
; them: top-left, top-right, bottom-left, bottom-right.
;
; Spawn and exit are drawn with the path tile and told apart by palette alone.
; That is readable on a colour Game Boy and invisible on a monochrome one, so
; they get their own glyphs before this ships.
cell_tiles:
  .byte TILE_GROUND, TILE_PATH, TILE_PATH, TILE_PATH
cell_tiles_end:

; ---- the side panel -------------------------------------------------------

; Six columns down the right of the screen. Drawn once, with the screen off; the
; numbers in it are rewritten every vertical blank by draw_status_numbers.
draw_hud:
  xor a
  ldh [VBK], a
  ld de, hud_rows
  ld hl, SCRN + HUD_COL
  ld c, HUD_ROWS
@row:
  push hl
  ld b, HUD_W
@char:
  ld a, [de]
  inc de
  ld [hl+], a
  dec b
  jr nz, @char
  pop hl
  ld a, l
  add 32
  ld l, a
  jr nc, @carried
  inc h
@carried:
  dec c
  jr nz, @row

  ld a, [wIsCgb]
  or a
  ret z
  ; The whole panel, every row of it, in the interface palette. Going down all
  ; eighteen rows rather than only the ones with words on them is what makes the
  ; panel read as a panel instead of as text floating beside the board.
  ld a, 1
  ldh [VBK], a
  ld hl, SCRN + HUD_COL
  ld c, 18
@attr_row:
  push hl
  ld b, HUD_W
@attr_col:
  ld a, PAL_UI
  ld [hl+], a
  dec b
  jr nz, @attr_col
  pop hl
  ld a, l
  add 32
  ld l, a
  jr nc, @attr_carried
  inc h
@attr_carried:
  dec c
  jr nz, @attr_row
  xor a
  ldh [VBK], a
  ret

; ---- the build cursor ------------------------------------------------------

move_cursor:
  ld a, [wJoyNew]
  bit 4, a               ; right
  jr z, @left
  ld b, a
  ld a, [wCurX]
  cp GRID_W - 1
  jr z, @skip_right
  inc a
  ld [wCurX], a
@skip_right:
  ld a, b
@left:
  bit 5, a               ; left
  jr z, @down
  ld b, a
  ld a, [wCurX]
  or a
  jr z, @skip_left
  dec a
  ld [wCurX], a
@skip_left:
  ld a, b
@down:
  bit 7, a               ; down
  jr z, @up
  ld b, a
  ld a, [wCurY]
  cp GRID_H - 1
  jr z, @skip_down
  inc a
  ld [wCurY], a
@skip_down:
  ld a, b
@up:
  bit 6, a               ; up
  ret z
  ld a, [wCurY]
  or a
  ret z
  dec a
  ld [wCurY], a
  ret

; The cursor is four objects: one corner bracket, and the same tile again with
; the object's X and Y flip bits set. A 16 by 16 cursor for sixteen bytes of
; tile data.
build_oam:
  ld hl, wOam
  ; A cell is eight pixels. An object's Y is its screen position plus sixteen and
  ; its X is its screen position plus eight, and the field starts sixteen pixels
  ; down under the status bar.
  ld a, [wCurY]
  add a
  add a
  add a
  add 16                 ; the object hardware's own Y offset; the field is at
  ld [hl+], a            ; the top of the screen now, so there is nothing else
  ld a, [wCurX]
  add a
  add a
  add a
  add 8
  ld [hl+], a
  ld a, TILE_CURSOR
  ld [hl+], a
  ld a, OPAL_CURSOR
  ld [hl+], a

  call oam_creeps

  ; Everything the creeps did not use is parked at Y=0, which is above the
  ; screen. The buffer is page aligned and 160 bytes, so the low byte alone says
  ; when we have reached the end of it.
@park:
  ld a, l
  cp <(wOam + 160)
  ret z
  xor a
  ld [hl+], a
  jr @park

; ---- creeps ----------------------------------------------------------------

clear_creeps:
  ld hl, wCreepAlive
  ld b, CREEP_MAX
@loop:
  xor a
  ld [hl+], a
  dec b
  jr nz, @loop
  ret

; Trickle creeps onto the board.
;
; This is a placeholder standing where the wave table goes. It exists so the
; route can be watched working, and it gets replaced by the schedule both
; cartridges derive from a shared seed.
wave_tick:
  ; A route with no step in it has nothing to walk along, and a map with no
  ; spawn produces exactly that. Better to send nothing than to send a creep to
  ; waypoint zero of a list that does not have one.
  ld a, [wPathLen]
  cp 2
  ret c
  ld hl, wSpawnTimer
  ld a, [hl]
  or a
  jr z, @due
  dec [hl]
  ret
@due:
  ld a, [wSpawnLeft]
  or a
  jr z, start_next_wave
  ld hl, wSpawnLeft
  dec [hl]
  ld a, SPAWN_GAP
  ld [wSpawnTimer], a
  ; fall through into spawn_creep

; Put one creep on the spawn, if there is a slot free. If there is not, it is
; simply not spawned: dropping a creep is a far better failure than writing past
; the end of the array.
spawn_creep:
  ld hl, wCreepAlive
  ld c, 0
@find:
  ld a, [hl+]
  or a
  jr z, @found
  inc c
  ld a, c
  cp CREEP_MAX
  jr nz, @find
  ret
@found:
  ld e, c
  ld d, 0
  ld hl, wCreepAlive
  add hl, de
  ld [hl], 1
  ld hl, wCreepIdx
  add hl, de
  ld [hl], 0
  ld hl, wCreepSub
  add hl, de
  ld [hl], 0
  ld hl, wCreepSpeed
  add hl, de
  call current_speed
  ld [hl], a
  ret

; a = the creep speed currently selected.
current_speed:
  push hl
  push de
  ld a, [wSpeedSel]
  ld e, a
  ld d, 0
  ld hl, creep_speeds
  add hl, de
  ld a, [hl]
  pop de
  pop hl
  ret

; Select steps the creep speed. Everything already on the board is changed too,
; so the effect is immediate rather than arriving with the next wave, which is
; the difference between a knob you can tune by eye and one you cannot.
cycle_speed:
  ld a, [wJoyNew]
  bit 2, a               ; select
  ret z
  ld hl, wSpeedSel
  inc [hl]
  ld a, [hl]
  cp CREEP_SPEEDS
  jr c, @apply
  xor a
  ld [hl], a
@apply:
  call current_speed
  ld hl, wCreepSpeed
  ld b, CREEP_MAX
@loop:
  ld [hl+], a
  dec b
  jr nz, @loop
  ret

; Slowest to fastest. Each is how much of a cell a creep crosses per frame out
; of 256, so 16 is a cell every sixteen frames.
creep_speeds:
  .byte 4, 8, 16, 24, 32, 48
creep_speeds_end:

; The wave is spent, so line the next one up. This is where the element draft
; will go, which is why the pause between waves is a named number rather than
; falling out of the spawn gap.
start_next_wave:
  ld a, WAVE_SIZE
  ld [wSpawnLeft], a
  ld a, WAVE_GAP
  ld [wSpawnTimer], a
  ld hl, wWave
  inc [hl]
  ret

; Advance every creep along the route.
;
; A creep is a waypoint index and a fraction of the way to the next one, so the
; carry out of one eight-bit addition is exactly "it has reached the next
; waypoint". A corner needs no special case at all, which is the reason the
; route is stored as waypoints rather than as pixel coordinates.
update_creeps:
  ld c, 0
@slot:
  ld e, c
  ld d, 0
  ld hl, wCreepAlive
  add hl, de
  ld a, [hl]
  or a
  jr z, @next

  ld hl, wCreepSpeed
  add hl, de
  ld a, [hl]
  ld b, a
  ld hl, wCreepSub
  add hl, de
  ld a, [hl]
  add b
  ld [hl], a
  jr nc, @next           ; still between the same two waypoints

  ld hl, wCreepIdx
  add hl, de
  inc [hl]
  ld b, [hl]
  ld a, [wPathLen]
  dec a                  ; the waypoint the exit sits on
  cp b
  jr nz, @next

  ; It reached the exit, so it is through and it costs a life.
  ld hl, wCreepAlive
  add hl, de
  ld [hl], 0
  ld hl, wLives
  ld a, [hl]
  or a
  jr z, @next
  dec [hl]

@next:
  inc c
  ld a, c
  cp CREEP_MAX
  jr nz, @slot
  ret

; One 8x8 object per live creep. hl is the object buffer write pointer and comes
; back advanced past whatever was written.
oam_creeps:
  ld c, 0
@slot:
  ld e, c
  ld d, 0
  push hl
  ld hl, wCreepAlive
  add hl, de
  ld a, [hl]
  pop hl
  or a
  call nz, oam_one_creep
  inc c
  ld a, c
  cp CREEP_MAX
  jr nz, @slot
  ret

; de = the creep's slot, hl = where in the object buffer it goes. Advances hl by
; four and leaves bc and de as it found them.
;
; The creep's pixel position is its waypoint's cell plus however far along the
; step to the next one it has got. Every step is axis aligned and exactly
; sixteen pixels, so that second part is an add or a subtract on ONE coordinate
; rather than a multiply on two.
oam_one_creep:
  push bc
  push de
  push hl

  ld hl, wCreepIdx
  add hl, de
  ld a, [hl]
  ld [wDrawWp], a
  ld hl, wCreepSub
  add hl, de
  ld a, [hl]
  srl a
  srl a
  srl a
  srl a
  srl a                  ; sub / 32: the sub-position across eight pixels
  ld [wDrawOff], a

  ; Where that waypoint is. A creep fills its cell now, so there is no centring
  ; to do: the offsets below are the object hardware's own and the status bar's.
  ld a, [wDrawWp]
  ld e, a
  ld d, 0
  ld hl, wPathCol
  add hl, de
  ld a, [hl]
  add a
  add a
  add a                  ; column * 8
  add 8                  ; the object's own X offset
  ld [wDrawX], a
  ld hl, wPathRow
  add hl, de
  ld a, [hl]
  add a
  add a
  add a                  ; row * 8
  add 16                 ; the object hardware's own Y offset
  ld [wDrawY], a

  ; And however far along the step it has got.
  ld hl, wPathDir
  add hl, de
  ld a, [hl]
  ld b, a
  ld a, [wDrawOff]
  ld c, a
  ld a, b
  or a
  jr nz, @not_right
  ld a, [wDrawX]
  add c
  ld [wDrawX], a
  jr @place
@not_right:
  cp 1
  jr nz, @not_down
  ld a, [wDrawY]
  add c
  ld [wDrawY], a
  jr @place
@not_down:
  cp 2
  jr nz, @not_left
  ld a, [wDrawX]
  sub c
  ld [wDrawX], a
  jr @place
@not_left:
  ld a, [wDrawY]
  sub c
  ld [wDrawY], a

@place:
  pop hl
  ld a, [wDrawY]
  ld [hl+], a
  ld a, [wDrawX]
  ld [hl+], a
  ld a, TILE_CREEP
  ld [hl+], a
  ld a, OPAL_CREEP
  ld [hl+], a
  pop de
  pop bc
  ret

; ---- the lives counter -----------------------------------------------------

; Two tiles, rewritten every vertical blank. Two writes is nothing, and doing it
; unconditionally means no part of the game has to remember to mark it dirty,
; which is the kind of thing that is always remembered until it is not.
draw_status_numbers:
  xor a
  ldh [VBK], a
  ld a, [wLives]
  ld hl, LIVES_AT
  call draw_number
  ld a, [wWave]
  ld hl, WAVE_AT
  call draw_number
  call current_speed
  ld hl, SPEED_AT
  call draw_number
  ret

; a = a number from 0 to 99, hl = where the first of its two digits goes.
; Repeated subtraction rather than a divide: it runs at most nine times, twice a
; frame, which is cheaper than any of the ways of not doing it.
draw_number:
  ld c, 0
@tens:
  cp 10
  jr c, @units
  sub 10
  inc c
  jr @tens
@units:
  ld b, a
  ld a, c
  add TILE_DIGIT0
  ld [hl+], a
  ld a, b
  add TILE_DIGIT0
  ld [hl], a
  ret

; ---- joypad ----------------------------------------------------------------

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
  cpl
  and $0f
  or b
  ld c, a

  ld a, %00110000        ; neither half selected
  ldh [P1], a

  ld a, [wJoy]
  cpl
  and c
  ld [wJoyNew], a
  ld a, c
  ld [wJoy], a
  ret

; ---- memory ----------------------------------------------------------------

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

; ---- palettes --------------------------------------------------------------

load_palettes:
  ld a, [wIsCgb]
  or a
  ret z
  ld hl, pal_ui
  ld a, PAL_UI
  call set_bg_pal
  ld hl, pal_ground
  ld a, PAL_GROUND
  call set_bg_pal
  ld hl, pal_path
  ld a, PAL_PATH
  call set_bg_pal
  ld hl, pal_spawn
  ld a, PAL_SPAWN
  call set_bg_pal
  ld hl, pal_exit
  ld a, PAL_EXIT
  call set_bg_pal
  ld hl, pal_cursor
  ld a, OPAL_CURSOR
  call set_obj_pal
  ld hl, pal_creep
  ld a, OPAL_CREEP
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

.include "data.s"
.include "tiles.s"

; ---- layout checks ---------------------------------------------------------
; Evaluated once every label is final, which is what makes them worth writing.

.assert tiles_end <= $7fff, "the cartridge has outgrown its 32 KB"
.assert dma_source_end - dma_source <= 16, "the DMA routine no longer fits in high RAM"
.assert (wOam & $ff) == 0, "the object buffer must start on a page boundary"
.assert wPathCol >= wCells + GRID_CELLS, "the route overlaps the board"

; Every tile number the code uses, tied to the picture it names. A constant that
; has drifted from its art is not something you find by reading either one.
.assert tile_ground - tiles_start == TILE_GROUND * 16, "TILE_GROUND is not where its art is"
.assert tile_path - tiles_start == TILE_PATH * 16, "TILE_PATH is not where its art is"
.assert tile_cursor - tiles_start == TILE_CURSOR * 16, "TILE_CURSOR is not where its art is"
.assert tile_creep - tiles_start == TILE_CREEP * 16, "TILE_CREEP is not where its art is"

; The cell tables have to have a row for every kind, or a decoded map indexes
; past the end of one of them and draws whatever follows it.
.assert cell_tiles_end - cell_tiles == CELL_KINDS, "cell_tiles has lost a kind"
.assert hud_rows_end - hud_rows == HUD_ROWS * HUD_W, "the panel is not HUD_W wide"
.assert GRID_W + HUD_W <= 20, "the board and the panel do not both fit across the screen"
.assert GRID_H <= 18, "the board is taller than the screen"
.assert creep_speeds_end - creep_speeds == CREEP_SPEEDS, "creep_speeds is not CREEP_SPEEDS long"
.assert cell_walkable_end - cell_walkable == CELL_KINDS, "cell_walkable has lost a kind"
.assert cell_palette_end - cell_palette == CELL_KINDS, "cell_palette has lost a kind"
