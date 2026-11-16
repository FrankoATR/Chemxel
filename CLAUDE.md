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
```

En este equipo Windows `cargo` está en `~/.cargo/bin` (puede no estar en el `PATH` del shell
Bash de la herramienta: `export PATH="$HOME/.cargo/bin:$PATH"`).

## Arquitectura

```
crates/
  mv_core       coords (WorldPos/ChunkPos/LocalPos), constantes, FixedTimestep, Config, paths
  mv_materials  registro de materiales (RON); reacciones en hito 5
  mv_world      Chunk 64³ por bricks, World (+colas), terrain, stream, raycast, edit, testgen
  mv_sim        simulación headless: autómata (hito 3); temperatura, reacciones (4–5)
  mv_physics    puente rapier3d                                          (hito 6)
  mv_render     wgpu: GpuContext, Camera, GpuVolume (ventana + brickmap), raymarch, blit
  mv_game       binario: App (loop), GameWorld (streaming+subida), Editor, Gui/overlay egui,
                FlyController, InputState, BenchFly
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
4. `Editor::update`: picking y pincel. Devuelve la caja editada → `Simulation::wake_box`.
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

**Tests:** `mv_sim/src/tests.rs` (mundo de 3×3×3 chunks con suelo de piedra). Toda regla
nueva lleva un test ahí.

### Render (`mv_render`, ADR 003 y 005)

**Pipeline:** `Renderer::render(&Camera, Option<BrushPreview>, overlay)`:
1. `GpuVolume::flush`.
2. Compute `raymarch.wgsl`: DDA de 3 niveles chunk → brick 8³ → vóxel, salida HDR
   `rgba16float`.
3. `blit.wgsl`: tonemap ACES a la swapchain sRGB.
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
`air` siempre es id 0. Esquema documentado en `crates/mv_materials/README.md`.

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
