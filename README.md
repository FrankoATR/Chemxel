# Chemxel

Sandbox de **microvóxeles** con física y química emergentes. Cada bloque de 1 m está
formado por 16×16×16 microvóxeles, y cada uno es de un material con propiedades propias:
la arena cae y se apila, el agua fluye, el calor se difunde, la madera arde, la piedra se
funde en lava y las estructuras sin soporte se desprenden.

Inspiración: Noita (simulación celular), Teardown (render y destrucción de vóxeles) y
Minecraft (mundo por chunks, construcción).

> La especificación completa (visión, arquitectura, hoja de ruta) está en
> [PROMPT.md](PROMPT.md).

## Estado

| Hito | Estado |
|---|---|
| 0 — Esqueleto | ✅ Completado |
| 1 — Chunk raymarcheado | ✅ Completado |
| 2 — Mundo y edición | ⬜ Pendiente |
| 3 — Autómata celular | ⬜ Pendiente |
| 4 — Temperatura | ⬜ Pendiente |
| 5 — Reacciones y fuego | ⬜ Pendiente |
| 6 — Integridad y rígidos | ⬜ Pendiente |
| 7 — Iluminación | ⬜ Pendiente |
| 8 — Jugador y guardado | ⬜ Pendiente |

Hoy el juego muestra un chunk de prueba (terreno sinusoidal con agua, una esfera de arena y
otra de lava) renderizado por raymarching en GPU, con cámara libre. La simulación corre a
30 Hz fijos, desacoplada del render. El detalle y las métricas de cada hito
están en la sección "Estado del proyecto" de [PROMPT.md](PROMPT.md#12-estado-del-proyecto).

## Requisitos

- [Rust](https://rustup.rs/) stable (el repo fija el canal con `rust-toolchain.toml`).
- Drivers de GPU con Vulkan, DirectX 12 o Metal.
- **Windows:** Visual Studio Build Tools con el componente "Desarrollo de escritorio con C++".
- **Linux:** `libx11-dev libxkbcommon-dev libwayland-dev` (o equivalentes).

## Compilar y ejecutar

```bash
cargo run --release              # ejecuta el juego
cargo test --workspace           # tests
cargo bench -p mv_benches        # benchmarks (Criterion)
./scripts/ci.sh                  # chequeos completos (Windows: .\scripts\ci.ps1)
```

Materiales: `assets/materials/*.ron` (esquema en
[crates/mv_materials/README.md](crates/mv_materials/README.md)).

Profiling con [Tracy](https://github.com/wolfpld/tracy):

```bash
cargo run --profile profiling -p mv_game --features profile
```

Logs: variable `RUST_LOG`, p. ej. `RUST_LOG=chemxel=debug cargo run --release`.
Backend gráfico forzado: `WGPU_BACKEND=dx12` (o `vulkan`).

## Configuración

`assets/config.ron` contiene los parámetros ajustables (ventana, vsync, tick rate, semilla,
distancia de render, campo de visión, memoria del brick pool, velocidad de la cámara y
sensibilidad del ratón). Todos los campos son opcionales. Para medir FPS reales, pon
`vsync: false`. Para usar otra carpeta de assets: variable de entorno `CHEMXEL_ASSETS_DIR`.

## Controles

| Tecla | Acción |
|---|---|
| `Click izq.` | Capturar el ratón (mirar con el ratón) |
| `WASD` | Moverse |
| `Espacio` / `Ctrl izq.` | Subir / bajar |
| `Shift` | Más rápido |
| `Esc` | Soltar el ratón; si ya está suelto, salir |

Controles previstos (hitos siguientes): `Click izq./der.` quitar/poner · `Rueda` radio del
pincel · `1–9` material · `F3` overlay de depuración · `F5` vista de temperatura.

## Estructura

```
crates/
  mv_core       tipos base: coordenadas, constantes, timestep fijo, configuración
  mv_materials  materiales y reacciones (RON)
  mv_world      chunks, almacenamiento, generación procedural
  mv_sim        simulación (autómata celular, temperatura, reacciones) — sin render
  mv_physics    cuerpos rígidos (rapier3d)
  mv_render     render con wgpu (brickmaps + raymarching)
  mv_game       binario del juego
benches/        benchmarks
assets/         configuración, materiales, reacciones, shaders
docs/decisions/ registro de decisiones de arquitectura (ADR)
```

## Stack

Rust · winit · wgpu (WGSL) · glam · rayon · serde + RON · egui · rapier3d · tracing + Tracy.
