# Changelog

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
