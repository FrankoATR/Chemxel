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
```

En este equipo Windows `cargo` está en `~/.cargo/bin` (puede no estar en el `PATH` del shell
Bash de la herramienta: `export PATH="$HOME/.cargo/bin:$PATH"`).

## Arquitectura

```
crates/
  mv_core       coords (WorldPos/ChunkPos/LocalPos), constantes, FixedTimestep, Config, paths
  mv_materials  registro de materiales y reacciones desde RON           (hito 3+)
  mv_world      chunks SoA, paleta, streaming, generación               (hito 1+)
  mv_sim        simulación headless: autómata, temperatura, reacciones  (hito 3+)
  mv_physics    puente rapier3d                                          (hito 6)
  mv_render     wgpu: GpuContext, Renderer, brickmaps, raymarching
  mv_game       binario: loop (tick fijo + render variable), input, cámara, egui
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

Cada frame: `FrameTimer::tick` → `FixedTimestep::advance(dt)` devuelve N ticks →
`Simulation::step()` N veces → `Renderer::render()`. El render nunca muta la simulación.
El `FixedTimestep` limita los ticks por frame y descarta el atraso (evita la espiral de muerte).

### Convenciones de datos

- Índice lineal en chunk: `x + y*64 + z*64²` (`LocalPos::index`). Iterar `z` fuera, `x` dentro.
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
  con métricas (FPS, ms/tick, benchmarks).

## Git

- Commits pequeños y descriptivos, en inglés, estilo `area: summary` (p. ej. `render: add brick pool`).
- **Sin trailers `Co-Authored-By`** ni atribución automática en los mensajes.
- No mezclar refactors grandes con features.
