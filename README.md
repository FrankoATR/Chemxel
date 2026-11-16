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
| 2 — Mundo y edición | ✅ Completado |
| 3 — Autómata celular | ✅ Completado |
| 4 — Temperatura | ⬜ Pendiente |
| 5 — Reacciones y fuego | ⬜ Pendiente |
| 6 — Integridad y rígidos | ⬜ Pendiente |
| 7 — Iluminación | ⬜ Pendiente |
| 8 — Jugador y guardado | ⬜ Pendiente |

Hoy el juego genera un mundo procedural infinito (colinas, montañas, playas y mar) que se
carga y descarga alrededor de la cámara en segundo plano, con 16 chunks (64 m) de distancia
de render. Se puede excavar y construir con un pincel esférico en tiempo real, y un overlay
de depuración (F3) muestra rendimiento, memoria y herramientas. La arena cae y forma
montículos, el agua fluye y se nivela, los gases suben; solo se simulan las zonas que
cambian, en paralelo, a 30 Hz fijos y desacoplado del render. El render es raymarching en
GPU sobre brickmaps.

Referencia (RTX 5060 Laptop, 1080p, sin vsync): ~700 FPS volando a 20 m/s sobre el
terreno, sin frames por encima de 16,7 ms. El detalle y las métricas de cada hito están en
la sección "Estado del proyecto" de [PROMPT.md](PROMPT.md#12-estado-del-proyecto).

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

Aceptación del autómata (100k vóxeles de arena y agua, ms por tick):

```bash
cargo bench -p mv_benches --bench sim_drop   # sin ventana, en una cuenca de piedra
cargo run --release -- --bench-drop          # en el juego, sobre el terreno real
```

Benchmark de streaming en el juego real (espera a que cargue el mundo, vuela en línea recta
y escribe en el log percentiles de tiempo de frame y memoria; usar con `vsync: false`):

```bash
cargo run --release -- --bench-fly        # 20 s; --bench-fly=60 para 60 s
```

Logs: variable `RUST_LOG`, p. ej. `RUST_LOG=chemxel=debug cargo run --release`.
Backend gráfico forzado: `WGPU_BACKEND=dx12` (o `vulkan`).

## Configuración

`assets/config.ron` contiene los parámetros ajustables. Todos los campos son opcionales:

- **Ventana:** tamaño, título.
- **Render:** vsync, distancia horizontal y vertical, memoria del brick pool, slots de
  chunk, presupuesto de subida por frame, campo de visión.
- **Mundo:** semilla, nivel del mar, forma del terreno, margen de descarga, trabajos de
  generación en paralelo.
- **Controles:** velocidad de la cámara, sensibilidad del ratón.
- **Pincel:** radio inicial y máximo, alcance, repeticiones por segundo. Para medir FPS reales, pon
`vsync: false`. Para usar otra carpeta de assets: variable de entorno `CHEMXEL_ASSETS_DIR`.

## Controles

| Tecla | Acción |
|---|---|
| `Click izq.` (ratón suelto) | Capturar el ratón (mirar con el ratón) |
| `WASD` | Moverse |
| `Espacio` / `Ctrl izq.` | Subir / bajar |
| `Shift` | Más rápido |
| `Click izq.` (mantener) | Quitar material (pincel esférico) |
| `Click der.` (mantener) | Poner el material seleccionado (no pisa sólidos ni polvos) |
| `Rueda` | Radio del pincel |
| `1`–`9` | Material (también desde el overlay, con el ratón suelto) |
| `F3` | Mostrar/ocultar el overlay de depuración (incluye el botón "Drop 100k sand + water") |
| `P` | Pausar/reanudar la simulación |
| `Esc` | Soltar el ratón; si ya está suelto, salir |

Previsto: `F5` vista de temperatura (hito 4).

## Estructura

```
crates/
  mv_core       tipos base: coordenadas, constantes, timestep fijo, configuración
  mv_materials  materiales y reacciones (RON)
  mv_world      chunks por bricks, mundo, terreno procedural, streaming, picking, edición
  mv_sim        simulación (autómata celular, temperatura, reacciones) — sin render
  mv_physics    cuerpos rígidos (rapier3d)
  mv_render     render con wgpu (brickmaps + raymarching)
  mv_game       binario del juego: loop, cámara, editor, overlay egui
benches/        benchmarks
assets/         configuración, materiales, reacciones, shaders
docs/decisions/ registro de decisiones de arquitectura (ADR)
```

## Stack

Rust · winit · wgpu (WGSL) · glam · rayon · serde + RON · egui · rapier3d · tracing + Tracy.
