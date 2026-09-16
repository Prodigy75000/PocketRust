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

GRID_W = 10
GRID_H = 8
GRID_CELLS = GRID_W * GRID_H
FIELD_ROW = 2                    ; the map row the field starts on
CELL_TILES = 2                   ; a cell is 2 by 2 tiles

; A cell row is two map rows, and the map is 32 tiles wide.
ROW_STRIDE = 32 * CELL_TILES

; Long enough for a path that visits most of the board, and a hard stop for a
; malformed map: the walk gives up here rather than writing past the list.
PATH_MAX = 64

; How many creeps may be on the board at once. Sixteen 8x8 objects plus the four
; the cursor uses is twenty of the forty the hardware has, which leaves room for
; projectiles and effects later without revisiting this number.
CREEP_MAX = 16

; A creep adds this to its position each frame, out of the 256 that make up one
; cell. Twelve is a cell every 21 frames, so about 2.8 cells a second.
CREEP_SPEED = 12

; The trickle that stands in for the wave table until there is one.
SPAWN_GAP = 40
WAVE_GAP = 150           ; the breather between waves, where the draft will go
WAVE_SIZE = 8

START_LIVES = 20

; Where the two-digit fields sit in the status bar, and the tile the digit zero
; is. A tile index is its character minus $20, and '0' is $30.
LIVES_COL = 17           ; on the first row
WAVE_COL = 6             ; on the second
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
TILE_BLANK     = $00             ; space, the first character of the font
TILE_GROUND_TL = $40
TILE_GROUND_TR = $41
TILE_GROUND_BL = $42
TILE_GROUND_BR = $43
TILE_PATH      = $44
TILE_CURSOR    = $45
TILE_CREEP     = $46

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
wPath:         .res PATH_MAX
wPathCol:      .res PATH_MAX
wPathRow:      .res PATH_MAX
wPathDir:      .res PATH_MAX    ; 0 right, 1 down, 2 left, 3 up, to the next one

.ram $c300
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
wWalkPrev:     .res 1    ; the cell it came from, so it cannot turn round
wWalkDir:      .res 1    ; which of the four neighbours is under test
wWalkTmp:      .res 1    ; the neighbour's index

wLives:        .res 1
wWave:         .res 1
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
.ram $c400
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
  call draw_status
  call build_path
  call clear_creeps
  ld a, START_LIVES
  ld [wLives], a
  ld a, 1
  ld [wWave], a
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
  ld c, GRID_CELLS
@cell:
  ld a, [de]
  inc de
  push de
  ld e, a
  ld d, 0
  push hl
  ld hl, cell_kind
  add hl, de
  ld a, [hl]
  pop hl
  pop de
  ld [hl+], a
  dec c
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

; a = cell kind, hl = the map address of the cell's top-left tile. Writes the
; four tiles, and on a colour Game Boy the four attributes behind them.
; Preserves hl, bc and de.
draw_cell:
  push hl
  push bc
  push de
  ld c, a
  ld b, 0
  ; de -> the four tile numbers for this kind
  push hl
  ld hl, cell_tiles
  add hl, bc
  add hl, bc
  add hl, bc
  add hl, bc
  ld d, h
  ld e, l
  pop hl
  xor a
  ldh [VBK], a
  call blit_quad
  ld a, [wIsCgb]
  or a
  jr z, @done
  push hl
  ld hl, cell_palette
  add hl, bc
  ld a, [hl]
  pop hl
  push hl
  push af
  ld a, 1
  ldh [VBK], a
  pop af
  ld [hl+], a
  ld [hl], a
  ld bc, 31
  add hl, bc
  ld [hl+], a
  ld [hl], a
  pop hl
  xor a
  ldh [VBK], a
@done:
  pop de
  pop bc
  pop hl
  ret

; de -> four bytes, hl = the map address of a cell's top-left tile. Writes them
; into the 2 by 2 block. Preserves hl and bc; de is left past the four.
blit_quad:
  push hl
  push bc
  ld a, [de]
  inc de
  ld [hl+], a
  ld a, [de]
  inc de
  ld [hl], a
  ld bc, 31
  add hl, bc
  ld a, [de]
  inc de
  ld [hl+], a
  ld a, [de]
  inc de
  ld [hl], a
  pop bc
  pop hl
  ret

; ---- the route -------------------------------------------------------------

; Walk the board from the spawn and write the ordered waypoint list into wPath.
;
; The picture in data.s is the only place the route is written down. This
; derives the order from it, so there is no second copy of the route to fall out
; of agreement with the first.
build_path:
  xor a
  ld [wPathLen], a
  ; Find the spawn, keeping the column and row rather than the index, so that
  ; nothing here ever needs to divide by ten.
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
  ld a, $ff
  ld [wWalkPrev], a
  ld hl, wPath
  ld b, 0                ; waypoints written so far

@step:
  ; wPath[b] is the cell the walk stands on. wPathCol and wPathRow are that same
  ; place as a column and a row, lifted from the walk's own state while it still
  ; has them, which is the whole reason nothing downstream needs to divide.
  call walk_index
  ld [hl+], a
  ld c, a                ; keep the cell index; the exit test below wants it
  push hl
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
  pop hl
  inc b
  ; Stop at the exit.
  ld e, c
  ld d, 0
  push hl
  ld hl, wCells
  add hl, de
  ld a, [hl]
  pop hl
  cp CELL_EXIT
  jr z, @done
  ; A malformed map must not write past the end of the list.
  ld a, b
  cp PATH_MAX
  jr nc, @done
  ; walk_next needs every register it can get, and this loop is holding the
  ; count in b and the write pointer in hl. Popping does not touch the flags, so
  ; the "did it move" answer survives being handed its registers back.
  push hl
  push bc
  call walk_next
  pop bc
  pop hl
  jr z, @done
  ; The direction it just took is the direction away from the waypoint written
  ; last, which is the one at b - 1. The last waypoint never gets one, because
  ; there is nowhere to go from the exit.
  push hl
  ld a, b
  dec a
  ld e, a
  ld d, 0
  ld hl, wPathDir
  add hl, de
  ld a, [wWalkDir]
  ld [hl], a
  pop hl
  jr @step

@done:
  ld a, b
  ld [wPathLen], a
  ret

; a = the cell index of wherever the walk currently stands. Preserves hl and de.
walk_index:
  push hl
  push de
  ld a, [wWalkY]
  ld e, a
  ld d, 0
  ld hl, row_base
  add hl, de
  ld a, [hl]
  ld hl, wWalkX
  add [hl]
  pop de
  pop hl
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
  ; A column of $FF is a step left and a column of ten is a step off the right
  ; edge. One unsigned comparison against the width rejects both, which is why
  ; the deltas are stored as bytes and not as a sign and a magnitude.
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
  ld e, b
  ld d, 0
  ld hl, row_base
  add hl, de
  ld a, [hl]
  add c
  ld [wWalkTmp], a
  ; Not the cell it came from.
  ld hl, wWalkPrev
  cp [hl]
  jr z, @next
  ; And somewhere a creep may walk.
  ld e, a
  ld d, 0
  ld hl, wCells
  add hl, de
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
  call walk_index
  ld [wWalkPrev], a
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
; Ground carries a tick in each of its four outer corners so that the board
; reads as 16-pixel cells rather than as an undifferentiated 8-pixel lattice.
; The path is flat on purpose: creeps move along it every frame, and a busy
; floor under a moving object is the fastest way to make a Game Boy unreadable.
cell_tiles:
  .byte TILE_GROUND_TL, TILE_GROUND_TR, TILE_GROUND_BL, TILE_GROUND_BR
  .byte TILE_PATH, TILE_PATH, TILE_PATH, TILE_PATH
  .byte TILE_PATH, TILE_PATH, TILE_PATH, TILE_PATH
  .byte TILE_PATH, TILE_PATH, TILE_PATH, TILE_PATH
cell_tiles_end:

; ---- the status bar --------------------------------------------------------

draw_status:
  ld hl, SCRN
  ld de, status_row0
  call blit_row
  ld hl, SCRN + 32
  ld de, status_row1
  call blit_row
  ld a, [wIsCgb]
  or a
  ret z
  ; Both rows, all 32 columns, in the interface palette.
  ld a, 1
  ldh [VBK], a
  ld hl, SCRN
  ld bc, 64
@attr:
  ld a, PAL_UI
  ld [hl+], a
  dec bc
  ld a, b
  or c
  jr nz, @attr
  xor a
  ldh [VBK], a
  ret

; de -> 20 characters, hl -> where they go. Bank 0.
blit_row:
  xor a
  ldh [VBK], a
  ld b, 20
@char:
  ld a, [de]
  inc de
  ld [hl+], a
  dec b
  jr nz, @char
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
  ; The field starts sixteen pixels down, an object's Y is its screen position
  ; plus sixteen, and an object's X is its screen position plus eight.
  ld a, [wCurY]
  swap a                 ; times sixteen: the cursor is always cell-aligned
  add 32
  ld d, a
  ld a, [wCurX]
  swap a
  add 8
  ld e, a

  ld a, d
  ld [hl+], a
  ld a, e
  ld [hl+], a
  ld a, TILE_CURSOR
  ld [hl+], a
  ld a, OPAL_CURSOR
  ld [hl+], a

  ld a, d
  ld [hl+], a
  ld a, e
  add 8
  ld [hl+], a
  ld a, TILE_CURSOR
  ld [hl+], a
  ld a, OPAL_CURSOR | OAM_XFLIP
  ld [hl+], a

  ld a, d
  add 8
  ld [hl+], a
  ld a, e
  ld [hl+], a
  ld a, TILE_CURSOR
  ld [hl+], a
  ld a, OPAL_CURSOR | OAM_YFLIP
  ld [hl+], a

  ld a, d
  add 8
  ld [hl+], a
  ld a, e
  add 8
  ld [hl+], a
  ld a, TILE_CURSOR
  ld [hl+], a
  ld a, OPAL_CURSOR | OAM_XFLIP | OAM_YFLIP
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
  ld a, CREEP_SPEED
  ld [hl], a
  ret

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
  swap a
  and $0f                ; the sub-position, in whole pixels
  ld [wDrawOff], a

  ; Where that waypoint is. A column is at most nine and a row at most seven, so
  ; `swap` multiplies either by sixteen with no masking needed.
  ld a, [wDrawWp]
  ld e, a
  ld d, 0
  ld hl, wPathCol
  add hl, de
  ld a, [hl]
  swap a
  add 12                 ; 8 for the object's own offset, 4 to centre it in the cell
  ld [wDrawX], a
  ld hl, wPathRow
  add hl, de
  ld a, [hl]
  swap a
  add 36                 ; 16 for the status bar, 16 for the object, 4 to centre
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
  ld hl, SCRN + LIVES_COL
  call draw_number
  ld a, [wWave]
  ld hl, SCRN + 32 + WAVE_COL
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
.assert wPath >= wCells + GRID_CELLS, "the route overlaps the board"

; Every tile number the code uses, tied to the picture it names. A constant that
; has drifted from its art is not something you find by reading either one.
.assert tile_ground_tl - tiles_start == TILE_GROUND_TL * 16, "TILE_GROUND_TL is not where its art is"
.assert tile_ground_tr - tiles_start == TILE_GROUND_TR * 16, "TILE_GROUND_TR is not where its art is"
.assert tile_ground_bl - tiles_start == TILE_GROUND_BL * 16, "TILE_GROUND_BL is not where its art is"
.assert tile_ground_br - tiles_start == TILE_GROUND_BR * 16, "TILE_GROUND_BR is not where its art is"
.assert tile_path - tiles_start == TILE_PATH * 16, "TILE_PATH is not where its art is"
.assert tile_cursor - tiles_start == TILE_CURSOR * 16, "TILE_CURSOR is not where its art is"
.assert tile_creep - tiles_start == TILE_CREEP * 16, "TILE_CREEP is not where its art is"

; The cell tables have to have a row for every kind, or a decoded map indexes
; past the end of one of them and draws whatever follows it.
.assert cell_tiles_end - cell_tiles == CELL_KINDS * 4, "cell_tiles has lost a kind"
.assert cell_walkable_end - cell_walkable == CELL_KINDS, "cell_walkable has lost a kind"
.assert cell_palette_end - cell_palette == CELL_KINDS, "cell_palette has lost a kind"
