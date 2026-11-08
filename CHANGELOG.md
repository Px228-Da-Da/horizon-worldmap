# Changelog

## 0.7.0

- **`/wp` commands** for waypoints from the chat: `/wp add [name]` (on a waypoint it renames it, so they
  never stack), `/wp del [name or number]` (nearest when none), `/wp rename <number> <name>`,
  `/wp list`, `/wp clear confirm`, and `/wp share <number> <player>`, which sends the place as a
  private message (only that player reads it, and it works on every server) or `/wp share <number> all`
  for the whole chat. A player who has the mod gets the same green [Add].
- **For Horizon Nightly `2ebf00a`** (2026-11-07). The mod's keys are now on its own page, Options > Mods
  > World Map, as the game lists the keys of a native mod (the newer mod tool no longer lets a mod add
  rows to the Key Binds screen).
- **Share a place with a friend.** *Share* in a waypoint's menu asks "Are you sure you want to
  share this waypoint WITH EVERYONE in the chat?" (Yes / No), then sends it as a line of plain
  words (`Waypoint "Home" at 100 64 -200 (overworld)`). A player who has the mod sees it as
  `Px226 shared a waypoint called "Home" from overworld! [Add]` with a green underlined **[Add]**;
  a click puts it on their map. Nothing is added without the click, and the click is the mod's
  own, nothing goes to the server. A player without the mod just reads the place.
- **Hotkeys in a waypoint's menu**, in front of the rows, as the player has the keys set (Edit E,
  Teleport T, Disable H, Delete DEL by default; Options > Mods > World Map). They work in the
  menu, and on the waypoint under the pointer without opening it. **Hide / show** a single
  waypoint is new.
- **A waypoint on the map** is its colour with its first letter; its name and place show when
  you point at it.
- **A big map no longer holds the game up.** Its files were all read and shaded at once when you
  entered the world (about half a second with 60 000 chunks on a fast computer, seconds on a slow one);
  now they are read a little in each frame, the chunks nearest to you first. The cells are kept
  by pointer (a growing map no longer copies every picture), a region is saved without going
  through every cell, and the count of explored chunks is not counted again in every frame.
- **Zoomed out, the map is drawn from its texels, not pixel by pixel** (from about 2 blocks to a pixel,
  when that is less work), so the cost follows what is explored, not the size of the screen. And the
  mod times its own drawing: on a slow computer the picture is drawn a little coarser (never on a fast
  one) so that moving the map does not hold the game up.
- **Zooming a big map is lighter.** While the map zooms its picture is drawn coarser (a ninth of
  the work) and sharp again as soon as it stops; the picture is drawn by up to four threads; and
  zoomed far out (more than a chunk to a pixel) it is made from the chunks there are, not from a
  window of the world that grew with the square of the zoom.
- **No flicker when zooming or turning.** The map was taken pixel by pixel and jumped between
  its sizes of detail (and the dithered forest shimmered) while zooming; and the minimap's picture
  crawled when it turned. They are taken smoothly now, between pixels and between sizes.
- **The chat tells what the map does:** `[Map] Waypoint Home set at 100 64 -200`, removed,
  added, shared.
- **Lighter on the game.** On busy servers (where blocks change all the time) the map read
  the whole loaded area again and again; now only the chunks near you are read again after a
  change, the others when they are new. It reads less in a frame (a fraction of a millisecond,
  3 ms with the map open), writes the map to disk by another thread, one region at a time,
  makes the list of other players four times a second instead of every frame, and does not
  work out the world's name or its scan every frame.

## 0.6.0

- **Minimap** in a corner of the game, with your waypoints and the night. It turns with you
  (the way you face is up) with the sides of the world on its edge, or keeps north up
  (a switch). Under a roof or in a cave it shows the floor at your height, cut where your
  head is, at every block of height, a jump too. Other players are white dots on it, bright at
  your height and dim above or below it. Its corner and size are in Options > Mods; keys `N`
  (on and off) and `Z` (scale).
- **Waypoints in the world** are a square of their colour with their first letter, and
  their name and distance under it. They are drawn even when the game does not hand its
  camera to mods (it hands it over only when it has registered mods), and follow the view
  smoothly while you walk, bob and turn, like the minimap.
- **Waypoint groups.** Give a waypoint a group in its menu; click a group's heading in the
  list to hide or show them all, on the map, the minimap and in the world.
- **Export and Import** of waypoints, buttons of the list: `exports/waypoints.txt` and
  `exports/import.txt` in the map's folder. The import also reads the waypoints file of
  other map mods (name, place, colour and group).
- **Smoother map:** it glides on when you let go of a drag, and it reads the world faster
  while it is open.
- **Fixed:** the waypoint's tip was drawn over the panels, and the cave panel covered the
  map's numbers.
- **Day and night on the map.** The surface of the Overworld is dark at night and light by
  day, following the sky; lit blocks keep their light, so torches, lava and towns show up in
  the dark. A switch in Options > Mods turns it off. Chunks seen before 0.6.0 have no light
  stored until you visit them again.
- **Fixed:** worlds with the same name ("New World", "New World (1)"...) shared one map and
  one set of waypoints, because the map was kept by the world's name. A single player world
  is now told by its seed too: the first world to use a folder keeps it, and a world with
  another seed gets its own (`sp-<name>-<seed>`). A friend's world (Play with Friends,
  reached at the same local address for every friend) is kept by its seed. The files of the
  worlds that were mixed up stay in `config/worldmap/sp-<name>/`.

## 0.5.0

First public release.

- **Self-writing full screen map**, kept per world, dimension and cave layer, in the vanilla
  map colours. A single player world is kept under its name.
- **Smooth** drag and zoom; far zoom blurs instead of flickering.
- **Cave layers** (layered or full) with a Top Y slider, -64 to 319.
- **Dimensions.** A button shows the Overworld, the Nether or the End from any of them, with
  the Nether and Overworld coordinates of the place under the pointer.
- **Waypoints:** add, rename, 16 colours, three shapes, teleport, delete, hide; markers in the
  world; a waypoint where you die; a list with search (L).
- **Also:** the biome under the pointer, other players on the map, go to coordinates (J),
  chunk grid, your trail, chunks still loading shown, PNG export, a settings page in
  Options > Mods.
- **Ten rebindable keys**, in Options > Controls > Key Binds under *World Map*, with Reset and
  marks for shared keys.
- **Russian** interface when the game is in Russian.
