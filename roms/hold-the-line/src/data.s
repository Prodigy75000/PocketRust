; SPDX-License-Identifier: CC0-1.0
; HOLD THE LINE. Dedicated to the public domain; see LICENSE.
;
; The maps, the palettes, and the lookup table that turns a picture of a map
; into cells.

; ---- maps ------------------------------------------------------------------
; Ten columns by eight rows, one character per 16x16 cell.
;
;   S  creeps enter here
;   E  creeps leave here, and it costs you a life
;   +  path
;   .  ground you can build on
;
; Map 1 takes creeps in at the top left and lets them out at the top RIGHT, the
; way Element TD does. Both ends being on the same edge is the constraint that
; decides the whole shape, and it rules out a spiral: a spiral has to finish
; somewhere in the middle, and there is no way back out to the rim from there
; without crossing an arm it already drew.
;
; What it leaves is a comb. Four vertical corridors, at columns 0, 3, 6 and 9,
; joined alternately at the bottom and the top, so the route runs down, up, down
; and out.
;
; The columns BETWEEN the corridors are the whole point: every one of them has a
; corridor on either side, so a tower standing anywhere in a gap covers two
; passes of the route at once. That is the decision the map exists to create.
;
; The corridors are three columns apart rather than two, which is the detail
; that took a wrong turn first. Two apart also works and is a cell shorter, but
; four corridors at columns 0, 2, 4 and 6 leave the whole right third of the
; board too far from anything to be worth building on, and a fifth corridor
; cannot be added: the route would then finish at the BOTTOM of the board, and
; there is no way back up to the top edge past a corridor it has already drawn.
; Three apart uses the full width and wastes nothing.
;
; The ORDER of the path is not written down anywhere. The cartridge walks it
; from S at load and builds the waypoint list itself, so there is no list of
; coordinates that can quietly stop agreeing with the picture above it.
;
; tools/checkmap.py proves the shape before it ships: one S, one E, every path
; cell with exactly two path neighbours except the two ends, and the walk from S
; reaching E having visited every path cell. A tower defence with a fork in its
; path is an easy bug to write and an impossible one to see by looking.

map_1:
  .str "S.........+++E"
  .str "+.+++.+++.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+.+.+.+.+.+..."
  .str "+++.+++.+++..."
map_1_end:

.assert map_1_end - map_1 == GRID_W * GRID_H, "map 1 is not ten by eight"

; ---- what a character means -------------------------------------------------
; Indexed by (character - $20), because that is exactly what .str emits. The font
; runs $20 to $5F, so the table is 64 bytes to spend four, and worth it: adding a
; cell kind is a one-byte edit here rather than another comparison in the
; decoder's inner loop.

cell_kind:
  ; $20 space, then ! " # $ % & ' ( ) * + , - . /
  .byte CELL_GROUND, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
  .byte CELL_PATH               ; $2B  +
  .byte 0, 0
  .byte CELL_GROUND             ; $2E  .
  .byte 0
  ; $30 0123456789 : ; < = > ?
  .byte 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
  ; $40 @ A B C D
  .byte 0, 0, 0, 0, 0
  .byte CELL_EXIT               ; $45  E
  ; $46 F G H I J K L M N O P Q R
  .byte 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
  .byte CELL_SPAWN              ; $53  S
  ; $54 T U V W X Y Z [ \ ] ^ _
  .byte 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
cell_kind_end:

.assert cell_kind_end - cell_kind == 64, "the character table no longer covers $20 to $5F"

; Which cell kinds a creep may walk on. Indexed by cell kind.
cell_walkable:
  .byte 0                       ; CELL_GROUND
  .byte 1                       ; CELL_PATH
  .byte 1                       ; CELL_SPAWN
  .byte 1                       ; CELL_EXIT
cell_walkable_end:

; The background palette each cell kind is drawn in, indexed by cell kind.
cell_palette:
  .byte PAL_GROUND, PAL_PATH, PAL_SPAWN, PAL_EXIT
cell_palette_end:

; ---- grid arithmetic --------------------------------------------------------
; Eight entries, so a cell row never needs a multiply and a cell index never
; needs a divide.

; Sixteen entries, so a cell row never needs a multiply. They are WORDS because
; a 20 by 16 board has 320 cells and row 15 starts at 300, which is not a byte.
row_base:
  .word 0, 14, 28, 42, 56, 70, 84, 98
  .word 112, 126, 140, 154, 168, 182, 196, 210
  .word 224

; ---- palettes ---------------------------------------------------------------
; Four fifteen-bit colours each, low byte first, in the order the palette port
; auto-increments through them.

pal_ui:
  .byte $41, $18, $8b, $41, $d5, $6a, $ff, $7f
pal_ground:
  .byte $83, $1c, $8a, $41, $0e, $4e, $d4, $66
pal_path:
  .byte $83, $1c, $4c, $19, $76, $36, $5c, $53
; Spawn and exit are the path in another colour. That is a readable difference
; on a colour Game Boy and no difference at all on a monochrome one, so they get
; their own glyphs before this ships. Noted here rather than forgotten.
pal_spawn:
  .byte $83, $1c, $c6, $19, $2b, $2f, $f4, $53
pal_exit:
  .byte $83, $1c, $8e, $10, $1b, $21, $5f, $42

pal_cursor:
  .byte $00, $00, $ff, $7f, $9f, $23, $ff, $53
pal_creep:
  .byte $00, $00, $ff, $7f, $bb, $35, $49, $08

; ---- the side panel --------------------------------------------------------
; Six characters per row, drawn down the right of the screen. The numbers here
; are placeholders: draw_status_numbers overwrites them every frame.

HUD_ROWS = 11

hud_rows:
  .str " LIFE "
  .str "   20 "
  .str "      "
  .str " WAVE "
  .str "   01 "
  .str "      "
  .str " SPD  "
  .str "   16 "
  .str "      "
  .str " GOLD "
  .str "  100 "
hud_rows_end:
