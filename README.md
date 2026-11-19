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
| 4 — Temperatura | ✅ Completado |
| 5 — Reacciones y fuego | ✅ Completado |
| 6 — Integridad y rígidos | ✅ Completado |
| 7 — Iluminación | ✅ Completado |
| 8 — Jugador y guardado | ✅ Completado |

Hoy el juego genera un mundo procedural infinito (colinas, montañas, playas y mar) que se
carga y descarga alrededor de la cámara en segundo plano, con 16 chunks (64 m) de distancia
de render. Se puede excavar y construir con un pincel esférico en tiempo real, y un overlay
de depuración (F3) muestra rendimiento, memoria y herramientas. La arena cae y forma
montículos, el agua fluye y se nivela, los gases suben. El calor se difunde y viaja con el
material: la lava funde el hielo, el agua hierve, el vapor sube y se condensa lejos, y la
lava se solidifica en piedra al enfriarse (vista de calor con F5). Hay química: la lava
que toca agua se apaga en obsidiana, el ácido disuelve la roca y el fuego se propaga por
madera y hojas, sube en llamas, deja ceniza y humo, y se apaga con agua. Lo que queda
suelto cae: al excavar la base de una torre, quemar un tronco o disolver un apoyo con
ácido, la pieza se convierte en un cuerpo rígido (rapier3d) que cae, vuelca, choca y,
al quedarse quieta, vuelve a formar parte del mundo. Solo se simulan las zonas que
cambian, en paralelo, a 30 Hz fijos y desacoplado del render. El render es raymarching en
GPU sobre brickmaps.

La imagen tiene sombras del sol, oclusión ambiental, lava y fuego que brillan e iluminan
lo que tienen cerca, agua, vapor y vidrio translúcidos, y niebla (F6 alterna la calidad).
Se camina con colisión (saltar, subir escalones, nadar) o se vuela (F), y el mundo se
guarda solo: al salir y volver a entrar todo sigue como estaba.

Referencia (RTX 5060 Laptop, 1080p, sin vsync): ~240 FPS volando a 20 m/s sobre el
terreno con toda la iluminación (~470 sin ella). El detalle y las métricas de cada hito están en
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

Materiales y reacciones: `assets/materials/*.ron` y `assets/reactions/*.ron` (esquema en
[crates/mv_materials/README.md](crates/mv_materials/README.md)). En builds de debug (o con
`--hot-reload`) el juego los vuelve a cargar al guardarlos; si un archivo no es válido, lo
dice en el log y sigue con la versión anterior.

Profiling con [Tracy](https://github.com/wolfpld/tracy):

```bash
cargo run --profile profiling -p mv_game --features profile
```

Aceptación del autómata (100k vóxeles de arena y agua, ms por tick):

```bash
cargo bench -p mv_benches --bench sim_drop   # sin ventana, en una cuenca de piedra
cargo run --release -- --bench-drop          # en el juego, sobre el terreno real
cargo bench -p mv_benches --bench sim_thermal # temperatura: lava sobre hielo, sin ventana
cargo run --release -- --bench-heat          # lo mismo en el juego, con la vista de calor
cargo bench -p mv_benches --bench sim_fire    # incendio de un bosque 6×6, sin ventana
cargo run --release -- --bench-fire          # lo mismo en el juego, sobre el terreno real
cargo run --release -- --bench-tower         # torre con la base excavada (cuerpos rígidos)
```

Para medir, el portátil debe estar enchufado: con batería la CPU baja de frecuencia y los
tiempos se duplican.

Benchmark de streaming en el juego real (espera a que cargue el mundo, vuela en línea recta
y escribe en el log percentiles de tiempo de frame y memoria; usar con `vsync: false`):

```bash
cargo run --release -- --bench-fly        # 20 s; --bench-fly=60 para 60 s
cargo run --release -- --bench-fly --lighting=off   # o shadows / full: coste de la iluminación
```

Partidas: `saves/<mundo>/` (o `CHEMXEL_SAVES_DIR`). Se guarda cada minuto y al salir.

```bash
cargo run --release -- --world=isla      # otra partida (por defecto: default)
cargo run --release -- --no-save         # no guardar nada esta vez
```

Los benchmarks no guardan, salvo que se indique `--world=`.

Logs: variable `RUST_LOG`, p. ej. `RUST_LOG=chemxel=debug cargo run --release`.
Backend gráfico forzado: `WGPU_BACKEND=dx12` (o `vulkan`).

## Configuración

`assets/config.ron` contiene los parámetros ajustables. Todos los campos son opcionales:

- **Ventana:** tamaño, título.
- **Render:** vsync, distancia horizontal y vertical, memoria del brick pool, slots de
  chunk, presupuesto de subida por frame, campo de visión, dirección del sol, distancia
  de sombras e intensidad de la oclusión ambiental.
- **Mundo:** semilla, nivel del mar, forma del terreno, margen de descarga, trabajos de
  generación en paralelo.
- **Controles:** velocidad de la cámara, sensibilidad del ratón.
- **Pincel:** radio inicial y máximo, alcance, repeticiones por segundo.
- **Jugador:** velocidad al caminar, salto, gravedad, altura de escalón, tamaño, empezar
  caminando o volando; cada cuántos segundos se autoguarda (`world.autosave_seconds`).
- **Integridad y física:** tamaño mínimo y máximo de las piezas que caen, profundidad del
  ancla bajo el terreno, gravedad, criterio de reposo, número máximo de cuerpos. Para medir FPS reales, pon
`vsync: false`. Para usar otra carpeta de assets: variable de entorno `CHEMXEL_ASSETS_DIR`.

## Controles

| Tecla | Acción |
|---|---|
| `Click izq.` (ratón suelto) | Capturar el ratón (mirar con el ratón) |
| `WASD` | Moverse |
| `Espacio` | Saltar (caminando; nadar hacia arriba en el agua) · subir (volando) |
| `Ctrl izq.` | Bajar (volando) |
| `Shift` | Correr / volar más rápido |
| `F` | Caminar ↔ volar (volando no hay colisión) |
| `Click izq.` (mantener) | Quitar material (pincel esférico) |
| `Click der.` (mantener) | Poner el material seleccionado (no pisa sólidos ni polvos) |
| `Rueda` | Radio del pincel |
| `1`–`9` | Material (también desde el overlay, con el ratón suelto) |
| `F3` | Mostrar/ocultar el overlay de depuración (con los escenarios "Drop 100k sand + water", "Lava on ice", "Forest fire" y "Tower collapse") |
| `P` | Pausar/reanudar la simulación |
| `F5` | Vista de calor (azul frío · gris ambiente · amarillo 100 °C · naranja · blanco) |
| `F6` | Calidad de iluminación: completa → solo sombras → sin iluminación |
| `Esc` | Soltar el ratón; si ya está suelto, salir |


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
