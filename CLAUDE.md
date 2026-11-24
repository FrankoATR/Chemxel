# CLAUDE.md

Guía para agentes que trabajan en este repositorio. La especificación completa del juego
está en [PROMPT.md](PROMPT.md): **léela antes de empezar un hito** y respeta su hoja de ruta.

## Qué es

Chemxel: sandbox de microvóxeles (16³ por bloque de 1 m) con física y química emergentes,
en Rust + `winit` + `wgpu` (WGSL). Sin motores (nada de Bevy/Godot); los vóxeles **nunca**
son entidades de ECS, viven en estructuras propias compactas (SoA).

## Comandos

```bash
cargo run --release                 # juego (binario `chemxel` del paquete mv_game)
cargo test --workspace
cargo bench -p mv_benches
./scripts/ci.sh                     # o .\scripts\ci.ps1 — fmt + clippy -D warnings + tests
cargo run --profile profiling -p mv_game --features profile   # con Tracy
RUST_LOG=chemxel=debug,mv_sim=trace cargo run --release       # logs (target del bin = `chemxel`)
cargo run --release -- --bench-fly[=SECS]   # benchmark de streaming en el juego (vsync: false)
cargo run --release -- --bench-drop[=SECS]  # 100k vóxeles soltados, ms/tick de la simulación
cargo bench -p mv_benches --bench sim_drop  # aceptación del autómata, sin ventana
cargo bench -p mv_benches --bench sim_thermal  # lava sobre hielo, sin ventana
cargo run --release -- --bench-heat[=SECS]  # lo mismo en el juego, con la vista F5
cargo bench -p mv_benches --bench sim_fire  # incendio de un bosque, sin ventana
cargo run --release -- --bench-fire[=SECS]  # lo mismo en el juego
cargo run --release -- --hot-reload         # recarga de RON también en release
cargo run --release -- --bench-tower[=SECS] # torre con la base excavada (rígidos)
cargo run --release -- --bench-fly --lighting=off  # (o shadows/full) coste de la iluminación
cargo run --release -- --world=NOMBRE       # partida en saves/NOMBRE (--no-save: no guardar)
cargo run --release -- --no-save --ui=inventory   # (o wheel) abre esa pantalla al arrancar
```

Los benchmarks no guardan salvo con `--world=`; para pruebas, `CHEMXEL_SAVES_DIR` apunta a
una carpeta temporal.

Benchmarks siempre con el portátil enchufado: con batería los tiempos se multiplican
×2–2,5. Los benchmarks del juego abren una ventana; si el usuario la mueve o toca, la
medición no vale.

En este equipo Windows `cargo` está en `~/.cargo/bin` (puede no estar en el `PATH` del shell
Bash de la herramienta: `export PATH="$HOME/.cargo/bin:$PATH"`).

## Arquitectura

```
crates/
  mv_core       coords (WorldPos/ChunkPos/LocalPos), constantes, FixedTimestep, Config, paths
  mv_materials  registro de materiales y reacciones (RON)
  mv_world      Chunk 64³ por bricks, World (+colas), terrain, stream, raycast, edit,
                persist (formato), store (disco), collide (caja del jugador), testgen
  mv_sim        simulación headless: autómata (3), temperatura (4), reacciones y fuego (5)
  mv_physics    cuerpos rígidos de vóxeles con rapier3d (hito 6)
  mv_render     wgpu: GpuContext, Camera, GpuVolume (ventana + brickmap), raymarch, blit
  mv_game       binario: App (loop), GameWorld (streaming+subida+guardado), Editor,
                Gui/overlay egui, Inventory (barra rápida, rueda Tab, catálogo Enter),
                Player, FlyController, SaveSlot, InputState, BenchFly
benches/        paquete mv_benches (Criterion)
assets/         config.ron, materials/*.ron, reactions/*.ron, shaders/*.wgsl
docs/decisions/ ADRs (NNN-titulo.md)
```

**Dirección de dependencias (obligatoria):**
`mv_core ← mv_materials ← mv_world ← mv_sim ← mv_physics`; `mv_render → mv_core, mv_world`;
`mv_game → todos`. `mv_sim` **nunca** depende de `mv_render` ni de `winit` (debe correr
headless). `mv_render` no depende de `winit` (recibe un `wgpu::SurfaceTarget`).

Las dependencias internas se declaran solo cuando se usan (cargo avisa de las no usadas y
la regla es cero warnings). Al añadir una, descomentarla también en
`[workspace.dependencies]` del `Cargo.toml` raíz. Ver ADR 002.

### Loop principal (`mv_game/src/app.rs`)

Cada frame:
1. `FrameTimer::tick`.
2. `FixedTimestep::advance(dt)` devuelve N ticks, y se llama `Simulation::step()` N veces.
3. Cámara.
4. `Editor::update`: picking y pincel. Devuelve un `Edit` → `Simulation::edited`.
5. `GameWorld::update`: streaming y subida a GPU (incluidos los chunks que tocó la
   simulación).
6. UI egui.
7. `Renderer::render(camera, brush, overlay)`.

El render nunca muta la simulación. El `FixedTimestep` limita los ticks por frame y
descarta el atraso (evita la espiral de muerte).

### Mundo (`mv_world`, ADR 004 y 005)

- **`Chunk`:** `Uniform(m)` o una tabla de 512 `Brick`, cada uno `Uniform(m)` o
  `Dense(Box<[MaterialId; 512]>)`.
  - `set` densifica solo el brick tocado; `compact` revierte.
  - `modified` indica si cambió desde que se generó.
- **`World`:** `HashMap<ChunkPos, Chunk>` más colas `loaded` (streaming), `edited` y
  `unloaded`. Quien consume (la subida a GPU) las vacía cada frame.
  - Para escribir vóxeles: `World::set_voxel`, o `chunk_mut` + `mark_edited`.
- **`terrain::TerrainGenerator`:** determinista por seed. Alturas por columna en caché y
  clasificación rápida de chunks/bricks uniformes. Un test compara contra la evaluación
  vóxel a vóxel.
- **`stream::Streamer`:** cilindro alrededor de la cámara, generación en rayon, resultados
  insertados en el hilo principal. Los chunks modificados que se descargan van a un stash.
- **`raycast`:** DDA en `f64`. **`edit::fill_sphere`:** edita chunk a chunk.

**Guardado (ADR 011):**
- **`persist`:** formato `CXCH` versionado: paleta de nombres + RLE varint + Adler-32.
- **`store::ChunkStore`:** un archivo por chunk modificado, escritura atómica.
- **Streaming:** `Streamer::set_store` hace que cargue del disco y escriba al descargar.
  `take_restored` lista los chunks devueltos, y la app llama a
  `Simulation::chunk_restored` (temperaturas base, despertar).
- **Marcas del `Chunk`:** `is_unsaved` / `mark_saved` (además de `is_modified`).
- **Partida** (`mv_game::save`):
  - `saves/<mundo>/world.ron` lleva versión, semilla y terreno (mandan sobre la config),
    tick y jugador.
  - Autoguardado sin cuerpos en vuelo; al salir, `Physics::settle_all` y guardar.
- **`collide::move_box`:** caja eje a eje con escalones; sin cargar = sólido. Lo usa
  `mv_game::player`.

### Simulación (`mv_sim`, ADR 006)

**Estado:** `Simulation::step(&mut World)` avanza un tick. Solo se procesan los chunks
activos (`HashMap<ChunkPos, ActiveChunk>`), y dentro de cada uno solo su región sucia:
- cada celda que cambia añade su entorno ±1 a la región pendiente de cada chunk que toca,
  incluidos los vecinos, que así se despiertan;
- un chunk se duerme tras `sleep_after_ticks` ticks sin cambios;
- las ediciones del jugador despiertan con `wake_box`.

**Fases en paralelo:** 8 fases por paridad 2×2×2. En cada una:
1. Se sacan sus chunks con `World::detach`.
2. Se procesan en rayon con `automaton::View`: el chunk propio en mutable y los 26
   vecinos en solo lectura.
3. Se devuelven con `World::attach`.
4. Se aplican en serie los `Move` entre chunks, validados.

Nada de `unsafe`.

**Reglas** (`automaton.rs`): todo movimiento es un intercambio ordenado por densidad
(`Sweep::enterable`); el barrido va de abajo arriba y alterna el sentido en x y z.
Aleatoriedad: `rng::hash(seed, tick, pos)`, sin estado. Los cambios marcan los chunks
como editados en `World`, y así se suben a la GPU.

**Temperatura (`thermal.rs`, ADR 007):**
- **Campo:** cada chunk apartado del ambiente tiene un `ThermalChunk` (16³ celdas de 4³
  con T, capacidad y conductividad).
- **Pase térmico** (`thermal_step`, tras el autómata):
  1. difusión Jacobi en paralelo;
  2. crecimiento a vecinos y sueño;
  3. transiciones por vóxel con histéresis, probabilidad y calor latente.
- **Advección:** `Sweep::swap` y `apply_move` mueven la energía con los vóxeles.
- **Ediciones:** `Simulation::edited(min, max, placed)` da la `base_temperature` al
  material colocado.
- **Energía:** la que no se conserva a propósito (cambios de fase a volumen constante,
  campos que se duermen) se acumula en `energy_imbalance`.

**Reacciones y fuego (ADR 008):**
- **Reacciones:** `a + b` se evalúa en el barrido al recorrer `a` (6 vecinos, matriz
  `MaterialTable::reaction`). Solo si algún compañero de `a` está en la máscara de
  materiales de los 27 chunks (`Chunk::materials_mask`). Las de borde van por la cola
  (`Move { react: true }`).
- **Productos:** `change_in_place` / `ThermalChunk::change_voxel` hacen la contabilidad de
  energía (latente + sensible); el `heat` extra va a `b`.
- **`lifetime`:** sin estado, `Props::decay` es la probabilidad por tick.
- **Vivos:** vóxeles que aún pueden reaccionar o caducar van a `TaskOut::live` para que el
  chunk no se duerma.
- **Combustión:** en `thermal::transitions` (`ThermalProps::burn`), con emisión de llamas.
- **Pérdida de calor:** `heat_loss` en `thermal::diffuse`.

**Integridad estructural (`integrity.rs`, ADR 009):**
- **Disparo:** cuando desaparece un sólido estático se encola una comprobación.
  - Ediciones (`edited` con `placed = None`): cajas.
  - Reacciones, combustión y fusión: puntos, procesados cada `integrity_natural_period`.
- **Relleno:** 6-conexo, primero en altura, con bitsets por chunk.
- **Anclado:** si llega al predicado `Anchor` (en el juego, `GameWorld::anchor`: bajo la
  superficie generada menos `anchor_depth`), a un chunk sin cargar, o si supera
  `integrity_max_voxels` / `integrity_max_extent`.
- **Resultado:** lo flotante (≥ `integrity_min_voxels`) sale como `VoxelBody` por
  `take_detached`.
- **Devolver vóxeles:** `voxels_placed` (con comprobación) o `voxels_restored` (sin
  ella).

**Tests:**
- `mv_sim/src/tests.rs`: reglas del autómata, en un mundo de 3×3×3 chunks con suelo de
  piedra.
- `tests_integrity.rs`: relleno, anclas, límites, presupuesto, piezas por reacción.
- `tests_thermal.rs` y `tests_chem.rs` fijan un ancla que lo ancla todo (no tratan de
  integridad).
- `tests_thermal.rs`: temperatura, con los materiales reales de `assets/materials`.
- `tests_chem.rs`: reacciones (materiales sintéticos para reglas exactas y los reales),
  incendio del bosque y energía.

Toda regla nueva lleva un test.

### Física (`mv_physics`, ADR 009)

- **Bucle:** `Physics::step(world, sim)` tras cada `Simulation::step`:
  1. toma las piezas;
  2. crea cuerpos rapier con colisionador `Voxels`;
  3. reconstruye por cuerpo el colisionador estático del mundo cercano (grupos de
     colisión para que solo toque a su cuerpo);
  4. avanza en subpasos (≤ medio vóxel por subpaso);
  5. revoxeliza los cuerpos quietos por mapeo inverso.
- **Unidades:** rapier en metros (vóxel = 1/16 m), relativo a un origen de física.
  `Physics::pose` da la pose en microvóxeles.
- **Render:** `Renderer::set_bodies(&[BodyInstance])` cada frame. Los cuerpos se trazan
  en `raymarch.wgsl` (bindings 8–9).
- **Tests:** `mv_physics/src/tests.rs`, de principio a fin (torre de aceptación, viga que
  vuelca, límite de cuerpos, caída en agua).

### Shaders

`mv_render` valida `raymarch.wgsl` y `blit.wgsl` con `wgpu::naga` en `cargo test`. Un error
de shader rompe la CI, no el arranque del juego.

### Render (`mv_render`, ADR 003 y 005)

**Pipeline:** `Renderer::render(&Camera, Option<BrushPreview>, overlay)`:
1. `GpuVolume::flush`.
2. Compute `raymarch.wgsl`: DDA de 3 niveles chunk → brick 8³ → vóxel, salida HDR
   `rgba16float`.
   - `trace(ro, rd, skip, t_limit)`: el mismo DDA para el rayo primario, la sombra
     y el rayo a través de translúcidos.
   - `shade()`: sol con sombra, AO (`corner_ao` + `ray_ambient`, que además recoge luz
     de emisivos), emisivos.
   - Una capa de transparencia (`material_run` mide el grosor).
   - Parámetros en `FrameUniform.lighting`, desde `Renderer::set_lighting`
     (ADR 010).
3. `blit.wgsl`: bloom barato y tonemap ACES a la swapchain sRGB.
4. Closure de overlay (egui) sobre una vista no sRGB de la swapchain.

**Datos GPU:**
- `chunk_grid`: ventana de chunks que sigue a la cámara (`set_volume_origin`).
- `chunk_bricks`: 512 entradas por slot de chunk.
- `brick_pool`: 512 × `u16` por brick, 2 por `u32`.
- Codificación de entradas: `EMPTY = u32::MAX` (aire), `UNIFORM_BIT | material`, o un
  slot.
- Los chunks son *residentes* con independencia de la ventana.

**Subida:**
- `upload_chunk` sube los bricks sucios; con slot nuevo sube los 512.
- `remove_chunk` libera.
- `GameWorld` prioriza: descargas → ediciones (sin presupuesto) → cargas (con
  `upload_budget_kib`).

**Precisión y structs compartidos:** la cámara guarda la posición en `f64`
(microvóxeles); la GPU recibe coordenadas relativas al origen de la ventana. Si cambias un
struct compartido con WGSL (`FrameUniform`, `MaterialGpu`), cambia también el shader.

### Materiales (`mv_materials`)

`MaterialRegistry::load_dir("assets/materials")` → tabla contigua por `MaterialId(u16)`;
`air` siempre es id 0. `ReactionSet::load_dir("assets/reactions", &registry)` → matriz de
reacciones. Esquemas documentados en `crates/mv_materials/README.md`.

- **Hot reload** (`mv_game/src/hot_reload.rs`): sondea los RON.
  `MaterialRegistry::keeps_ids_of` rechaza los cambios que renumeran.
  `Simulation::set_materials`, `Editor::set_materials` y `Renderer::set_materials`
  aplican la recarga.
- **Ids:** no reordenar ni borrar materiales; añadir siempre al final.

### Convenciones de datos

- Todos los índices son x-rápido. Almacenamiento del chunk: `LocalPos::brick_index`
  (brick dentro del chunk) + `LocalPos::brick_local_index` (vóxel dentro del brick).
  `LocalPos::index` (`x + y*64 + z*64²`) es el índice plano del chunk entero. Iterar `z`
  fuera, `x` dentro.
- Conversión mundo↔chunk por shifts/máscaras (`>> 6`, `& 63`); correcta con negativos.
- Constantes de escala en `mv_core::consts`. Constantes de tuning en `assets/config.ron`
  (nunca hardcodear tick rate, radios, presupuestos).

## Reglas

- Código, identificadores y comentarios en **inglés**; documentación de usuario (README,
  ADRs, notas de hitos) en **español**.
- `cargo fmt` + `cargo clippy --workspace --all-targets -- -D warnings` + tests en verde
  antes de cada commit. `unsafe` prohibido salvo justificado (`#[allow(unsafe_code)]` + `// SAFETY:`).
- Rutas calientes: nada de `HashMap` por vóxel, nada de asignaciones en el loop interno,
  datos contiguos, iteración en orden de memoria. Spans de profiling en rutas calientes:
  `trace_span!`.
- Toda regla de simulación lleva tests en grids pequeños y deterministas (RNG por
  seed + tick + posición).
- Nueva dependencia externa fuera de la lista de PROMPT.md §3 → justificarla en el commit
  (y en un ADR si es relevante).
- Decisión de arquitectura no cubierta por PROMPT.md → la opción más simple que cumpla
  los presupuestos, documentada en `docs/decisions/NNN-titulo.md`.
- Si algo de PROMPT.md resulta inviable: parar, medir, explicar y proponer alternativas.
- Al cerrar un hito: actualizar la tabla "Estado del proyecto" de PROMPT.md y el README,
  con métricas (FPS, ms/tick, benchmarks; `--bench-fly` para el frame time real).

## Git

- Commits pequeños y descriptivos, en inglés, estilo `area: summary` (p. ej. `render: add brick pool`).
- **Sin trailers `Co-Authored-By`** ni atribución automática en los mensajes.
- No mezclar refactors grandes con features.
