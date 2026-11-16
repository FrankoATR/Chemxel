# MicroVox — Sandbox de microvóxeles con física y química emergentes

> **Este documento es el prompt maestro para un agente de Claude Code.**
> Léelo completo antes de escribir código. Trabaja **hito por hito** (ver "Hoja de ruta"), no avances al siguiente hasta cumplir los criterios de aceptación del actual, y al terminar cada hito actualiza la sección "Estado del proyecto" al final de este archivo.

---

## 1. Visión

Un juego sandbox estilo Minecraft donde cada bloque clásico está compuesto por **16×16×16 microvóxeles** (4096 por bloque). Cada microvóxel es de un **material** con propiedades físicas y químicas propias, y reacciona con su entorno:

- La arena cae y se apila; el agua fluye y llena huecos; el vapor sube.
- El calor se difunde: el hielo se derrite, el agua hierve, la madera arde, la piedra se funde en lava.
- Los materiales reaccionan entre sí (agua + lava → vapor + obsidiana; fuego + madera → ceniza + humo).
- Las estructuras sin soporte se desprenden y caen como cuerpos rígidos.

Referencias de diseño: **Noita** (simulación celular por píxel), **Teardown** (render y destrucción de microvóxeles), **Minecraft** (mundo infinito por chunks, construcción).

**Plataforma objetivo:** PC (Windows y Linux primero, macOS después). Arquitectura preparada para multiplataforma (web/WebGPU) a futuro, sin trabajar en eso todavía.

---

## 2. Escala y unidades

| Concepto | Valor |
|---|---|
| 1 bloque "Minecraft" | 1 m = 16 microvóxeles por eje |
| 1 microvóxel | 6,25 cm |
| Chunk de simulación/almacenamiento | **64³ microvóxeles** (= 4×4×4 bloques, 262 144 vóxeles) |
| Brick de render | **8³ microvóxeles** (512 por brick, 512 bricks por chunk) |
| Distancia de render inicial | ~16 chunks (≈ 64 bloques) — ajustable |
| Burbuja de simulación activa | ~4–6 chunks alrededor del jugador + chunks despertados por eventos |

**Regla de oro:** el mundo está *dormido* por defecto. Solo se simula lo que está activo. Simular todo el mundo es imposible a esta escala y no es un objetivo.

---

## 3. Stack tecnológico (decidido)

- **Lenguaje:** Rust (edición 2021 o superior, toolchain stable).
- **Ventana/input:** `winit`
- **GPU:** `wgpu` (Vulkan / DX12 / Metal / WebGPU), shaders en **WGSL**.
- **Matemáticas:** `glam`
- **Paralelismo:** `rayon`
- **Física de cuerpos rígidos:** `rapier3d` (a partir del hito 6).
- **Datos/config:** `serde` + **RON** para materiales y reacciones.
- **UI de depuración:** `egui` (+ `egui-wgpu`, `egui-winit`).
- **Ruido procedural:** `noise` o `fastnoise-lite`.
- **Profiling:** `tracing` + `tracy-client` (detrás de un feature flag `profile`).
- **Logging:** `log` + `env_logger` (o `tracing-subscriber`).
- **Tests/benchmarks:** `cargo test`, `criterion`.

**No usar** motores completos (Bevy, Godot, Unity, Unreal). Los vóxeles **nunca** son entidades de ECS; viven en estructuras de datos propias y compactas.

Antes de añadir cualquier dependencia nueva fuera de esta lista, justifícala en el commit.

---

## 4. Estructura del repositorio (workspace de Cargo)

```
microvox/
├── Cargo.toml                # workspace
├── README.md                 # este archivo
├── assets/
│   ├── materials/            # *.ron: definición de materiales
│   ├── reactions/            # *.ron: tabla de reacciones
│   └── shaders/              # *.wgsl
├── crates/
│   ├── mv_core/              # tipos base, coordenadas, matemáticas, config
│   ├── mv_world/             # chunks, almacenamiento, paleta, streaming, generación
│   ├── mv_sim/               # autómata celular, temperatura, reacciones, integridad
│   ├── mv_materials/         # carga y registro de materiales/reacciones desde RON
│   ├── mv_render/            # wgpu, brickmaps, raymarching, iluminación
│   ├── mv_physics/           # puente con rapier3d (cuerpos rígidos de vóxeles)
│   └── mv_game/              # binario: loop principal, input, cámara, UI debug
└── benches/                  # benchmarks criterion
```

Las dependencias entre crates van en una sola dirección:
`mv_core ← mv_materials ← mv_world ← mv_sim ← mv_physics`, `mv_render` depende de `mv_core` y `mv_world`, y `mv_game` depende de todos. **`mv_sim` no puede depender de `mv_render`** (la simulación debe poder correr headless para tests y, a futuro, en un servidor).

---

## 5. Arquitectura de datos

### 5.1 Coordenadas
- `WorldPos` (i64 x3) en microvóxeles, `ChunkPos` (i32 x3) y `LocalPos` (u8 x3, 0..64).
- Conversión por desplazamiento de bits (64 = 2⁶). Implementar con tests exhaustivos, incluyendo coordenadas negativas.

### 5.2 Chunk (Structure of Arrays)
```rust
pub struct Chunk {
    pub material: Box<[MaterialId; 64*64*64]>, // MaterialId = u16; 0 = aire
    pub temperature: Box<[f16 o u16 cuantizado; ...]>, // puede ser grilla más gruesa (ver 6.3)
    pub flags: Box<[u8; ...]>,      // bits: actualizado_este_tick, ardiendo, etc.
    pub state: ChunkState,          // Dormido | Activo | Descargando...
    pub dirty_bricks: BitSet<512>,  // bricks a resubir a la GPU
    pub active_region: Option<Aabb>,// AABB de celdas que cambiaron (dirty rect 3D)
}
```
- Índice lineal: `x + y*64 + z*64*64` (definir y documentar un orden, y usarlo siempre).
- Chunks uniformes (todo aire, todo piedra) se representan sin arrays (`Chunk::Uniform(MaterialId)`) para ahorrar memoria.
- Compresión en disco: paleta por chunk + RLE o LZ4. Formato de guardado versionado.

### 5.3 Registro de materiales
Un `MaterialRegistry` cargado desde `assets/materials/*.ron` al iniciar, con recarga en caliente (hot reload) en modo debug. Las propiedades se acceden por `MaterialId` en una tabla contigua (cache-friendly).

---

## 6. Simulación (`mv_sim`) — el corazón del juego

### 6.1 Tick
- Tick de simulación fijo: **30 Hz** por defecto (configurable), desacoplado del framerate de render.
- Orden de cada tick:
  1. Movimiento celular (polvos, líquidos, gases)
  2. Difusión de temperatura
  3. Transiciones de estado (fusión, ebullición, congelación, combustión)
  4. Reacciones químicas entre vecinos
  5. Chequeo de integridad estructural (en las regiones modificadas)
  6. Actualizar chunks activos/dormidos y marcar bricks sucios

### 6.2 Autómata celular 3D
Comportamiento según `state` del material:
- **Solid estático:** no se mueve (piedra, madera). Puede soltarse por integridad estructural.
- **Powder:** cae; si abajo está ocupado, intenta las 4 diagonales inferiores en orden aleatorio. Puede desplazar líquidos/gases de menor densidad.
- **Liquid:** cae, luego diagonales inferiores, luego se dispersa horizontalmente hasta `dispersion` celdas. Intercambio por densidad (el aceite flota sobre el agua).
- **Gas:** como el líquido pero hacia arriba, con movimiento aleatorio y `lifetime` opcional.
- **Fire/Energía:** materiales especiales que se consumen y propagan calor.

Reglas de implementación:
- Aleatoriedad con un RNG rápido y **determinista por seed + tick + posición** (p. ej. hash de las coordenadas). Esto facilita tests reproducibles y deja abierta la puerta al multijugador.
- Bit `updated_this_tick` para que un vóxel no se mueva dos veces en el mismo tick.
- Alternar el orden de barrido horizontal cada tick para evitar sesgos direccionales.

### 6.3 Temperatura
- Campo escalar por vóxel **o** en grilla gruesa (p. ej. 1 celda de temperatura por 2³ o 4³ vóxeles). Empezar con la grilla gruesa 4³ por rendimiento y medir.
- Difusión explícita con conductividad por material, ponderada por la capacidad calorífica, estable (respetar la condición CFL / limitar el paso).
- Fuentes de calor: lava, fuego, reacciones exotérmicas.

### 6.4 Transiciones de estado
Definidas por material: `melt_at`/`melts_into`, `boil_at`/`boils_into`, `freeze_at`/`freezes_into`, `ignite_at`/`burns_into`, más el calor latente opcional.

### 6.5 Reacciones químicas
- Tabla cargada desde `assets/reactions/*.ron`.
- Al evaluar un vóxel activo, se revisan sus 6 vecinos (o 26 si hace falta) y se busca en la tabla `(material_a, material_b)` con un lookup O(1) (`HashMap` precomputado o una matriz dispersa).
- Cada reacción tiene: probabilidad por tick, rango de temperatura requerido, productos para A y B, y calor liberado o absorbido.

### 6.6 Integridad estructural
- Al quitar vóxeles sólidos, hacer flood fill en la región afectada para encontrar componentes conectados.
- Un componente sin conexión con el "suelo" (material ancla o borde inferior del mundo) se extrae del grid y se convierte en un **cuerpo rígido** (hito 6).
- Al estabilizarse, el cuerpo rígido se vuelve a voxelizar en el grid.
- Limitar el flood fill por presupuesto de tiempo y por tamaño máximo; si se excede, diferirlo a ticks siguientes.

### 6.7 Paralelismo y chunks activos
- Solo se procesan chunks en estado `Activo`. Un chunk se duerme tras N ticks sin cambios y se despierta si un vecino escribe en él o si el jugador lo modifica.
- Dentro de cada chunk, procesar solo la `active_region` (AABB sucio, expandido 1 celda).
- Actualización multihilo con **patrón de tablero de ajedrez 3D en 8 fases** (2×2×2): en cada fase se procesan en paralelo (rayon) chunks que no son vecinos entre sí, así las escrituras que cruzan bordes son seguras sin locks.
- Movimiento entre chunks: acceso a vecinos mediante una "vista" de 3×3×3 chunks que se pasa a cada tarea.

### 6.8 Presupuesto de rendimiento (objetivo inicial)
- Simulación ≤ **8 ms por tick** en un CPU de 8 núcleos con ~50 chunks activos.
- Si se excede: reducir la frecuencia de los chunks lejanos (simulación por LOD temporal) antes que bajar la calidad cerca del jugador.

---

## 7. Render (`mv_render`)

### 7.1 Enfoque: raymarching en GPU sobre brickmaps
El greedy meshing clásico no escala a microvóxeles. En su lugar:
- **Nivel 1 (grid de chunks):** textura o buffer con el índice de cada chunk cargado (o "vacío").
- **Nivel 2 (bricks 8³):** por chunk, una máscara de ocupación de 512 bits y punteros a los bricks no vacíos dentro de un **pool global** en la GPU (buffer o textura 3D atlas).
- **Nivel 3 (vóxeles):** dentro del brick, material u16 (o u8 vía paleta).
- Recorrido con **DDA jerárquico** (Amanatides & Woo): saltar chunks vacíos, luego bricks vacíos, luego vóxeles.
- Compute shader que escribe a una textura de color + profundidad, luego un blit/tonemap a la swapchain.

### 7.2 Subida incremental
- Solo se resuben los bricks marcados en `dirty_bricks`. Usar staging buffers y `queue.write_buffer`.
- Presupuesto de subida por frame (p. ej. 4–8 MB) para evitar picos.

### 7.3 Iluminación (progresiva)
1. Hito 1: color por material + sombreado por normal (de la cara del vóxel que golpea el rayo).
2. Hito 7: sombras direccionales (rayo secundario hacia el sol), AO por rayos cortos, emisión (lava/fuego brillan).
3. Futuro: GI de 1 rebote con acumulación temporal.

### 7.4 Requisitos
- Cámara libre (fly) en primera persona: WASD + mouse, velocidad ajustable.
- Picking de vóxel bajo el cursor usando el mismo DDA en CPU, para editar.
- Overlay `egui`: FPS, ms de sim/render, chunks cargados/activos, memoria GPU, material seleccionado, herramientas de pincel.

---

## 8. Formato de materiales y reacciones (RON)

`assets/materials/basic.ron`:
```ron
[
  Material(name: "air",   state: Gas,    density: 1.2,   color: (0,0,0,0),        conductivity: 0.02),
  Material(name: "stone", state: Solid,  density: 2600,  color: (120,120,125,255), conductivity: 2.5,
           heat_capacity: 0.8, melt_at: 1200, melts_into: "lava"),
  Material(name: "sand",  state: Powder, density: 1600,  color: (220,200,140,255), conductivity: 0.3,
           melt_at: 1700, melts_into: "glass"),
  Material(name: "water", state: Liquid, density: 1000,  color: (40,90,200,180),   conductivity: 0.6,
           heat_capacity: 4.18, dispersion: 5,
           boil_at: 100, boils_into: "steam", freeze_at: 0, freezes_into: "ice"),
  Material(name: "steam", state: Gas,    density: 0.6,   color: (220,220,230,90),  lifetime: Some((200, 600)),
           freeze_at: 99, freezes_into: "water"),
  Material(name: "lava",  state: Liquid, density: 3100,  color: (255,90,20,255),   emissive: 4.0,
           dispersion: 1, base_temperature: Some(1300), freeze_at: 700, freezes_into: "stone"),
  Material(name: "wood",  state: Solid,  density: 700,   color: (130,85,45,255),   flammable: true,
           ignite_at: 300, burns_into: "ash", burn_rate: 0.02),
]
```

`assets/reactions/basic.ron`:
```ron
[
  Reaction(a: "water", b: "lava", result_a: "steam", result_b: "obsidian", probability: 0.3, heat: -200),
  Reaction(a: "fire",  b: "wood", result_a: "fire",  result_b: "fire",     probability: 0.05, heat: 50),
  Reaction(a: "acid",  b: "stone",result_a: "air",   result_b: "air",      probability: 0.02),
]
```
El esquema exacto puede evolucionar, pero debe estar documentado en `crates/mv_materials/README.md` y validarse al cargar (con errores claros si un nombre referenciado no existe).

---

## 9. Hoja de ruta (hitos)

Cada hito termina con: el código compila sin warnings (`cargo clippy -- -D warnings`), los tests pasan, un commit (o una serie de commits) con un mensaje claro, y la actualización de "Estado del proyecto".

### Hito 0 — Esqueleto
- Workspace con todos los crates vacíos, CI local (`cargo fmt --check`, `clippy`, `test`).
- Ventana con `winit` + `wgpu` que limpia la pantalla con un color. Loop principal con tick fijo + render variable.
- **Aceptación:** `cargo run --release` abre una ventana estable; el log muestra el adaptador GPU usado.

### Hito 1 — Un chunk raymarcheado
- `Chunk` 64³ con datos de prueba (esfera, terreno sinusoidal).
- Brickmap de un chunk en la GPU + compute shader de raymarching DDA + cámara fly.
- **Aceptación:** se ve el chunk con colores por material y sombreado por normal a ≥ 144 FPS en 1080p (GPU de gama media).

### Hito 2 — Mundo por chunks y edición
- Grid de chunks, carga/descarga por distancia, generación procedural de terreno (ruido) con capas de piedra/tierra/arena/agua.
- Picking y pincel esférico para poner/quitar material (radio ajustable, material seleccionable desde egui).
- Subida incremental de bricks sucios.
- **Aceptación:** volar por un terreno de ≥ 16 chunks de radio sin tirones; editar en tiempo real se refleja en el mismo frame o el siguiente.

### Hito 3 — Autómata celular básico
- `MaterialRegistry` desde RON. Estados Powder, Liquid, Gas, Solid.
- Chunks activos/dormidos, dirty AABB, tablero de ajedrez 3D con rayon.
- **Aceptación:** soltar 100 000 vóxeles de arena y agua: la arena forma montículos, el agua se nivela, todo en ≤ 8 ms/tick. Tests unitarios de cada regla de movimiento en grids pequeños, deterministas por seed.

### Hito 4 — Temperatura y transiciones
- Campo de temperatura (grilla gruesa), difusión, fuentes de calor.
- Transiciones de estado (hielo↔agua↔vapor, piedra→lava, lava→piedra).
- Vista de debug con un mapa de calor.
- **Aceptación:** lava sobre hielo produce agua y luego vapor; el vapor sube y se condensa lejos de la fuente. Test de conservación aproximada de energía en un sistema cerrado.

### Hito 5 — Reacciones químicas y fuego
- Tabla de reacciones, fuego que se propaga por madera, humo, ceniza, ácido.
- **Aceptación:** un bosque de madera arde de forma creíble; agua + lava → obsidiana en la interfaz. Hot reload de los archivos RON en modo debug.

### Hito 6 — Integridad estructural y cuerpos rígidos
- Flood fill acotado, extracción de componentes sueltos, `rapier3d` con colisionadores derivados de los vóxeles (cajas por brick o convex decomposition simplificada).
- Render de los cuerpos rígidos (sus propios brickmaps con transformación).
- Revoxelizar al dormirse.
- **Aceptación:** excavar la base de una torre la hace caer como un bloque; al chocar, queda integrada al mundo.

### Hito 7 — Iluminación y pulido visual
- Sombras del sol, AO, emisivos, transparencia simple para agua/vapor, niebla.
- **Aceptación:** la escena se ve "Teardown-like" manteniendo ≥ 60 FPS.

### Hito 8 — Jugador y persistencia
- Controlador de personaje con colisión contra vóxeles, modo de vuelo/caminar.
- Guardado/carga del mundo (chunks modificados comprimidos, formato versionado).
- **Aceptación:** salir y volver a entrar conserva todos los cambios.

### Futuro (no empezar sin pedirlo)
Inventario/crafting, mobs, multijugador (servidor autoritativo con sim headless), build WebGPU, soporte de mods vía archivos RON/scripts.

---

## 10. Convenciones para el agente

- **Idioma:** código, identificadores y comentarios de código en **inglés**; documentación para el usuario (README, notas de hitos) en **español**.
- **Calidad:** `cargo fmt`, `cargo clippy -- -D warnings`, sin `unsafe` salvo justificado y comentado (`// SAFETY:`).
- **Tests:** toda regla de simulación tiene tests en grids pequeños y deterministas. Las conversiones de coordenadas tienen tests de bordes y negativos.
- **Benchmarks:** `criterion` para el tick de simulación, la subida de bricks y la generación de chunks. Reportar los números al cerrar cada hito.
- **Rendimiento primero en las rutas calientes:** nada de `HashMap` por vóxel, nada de asignaciones de memoria en el loop interno, datos contiguos, iteración en el orden de memoria.
- **Configurable:** constantes de tuning (tick rate, radio de render, presupuestos) en un `config.ron`, no hardcodeadas.
- **Commits pequeños y descriptivos.** No mezclar refactors grandes con features.
- **Ante una decisión de arquitectura no cubierta aquí:** elegir la opción más simple que cumpla los presupuestos de rendimiento, documentarla en `docs/decisions/NNN-titulo.md` (formato ADR corto) y seguir.
- **Si algo de este documento resulta inviable** (rendimiento, limitación de wgpu, etc.), detenerse, explicar el problema con datos (mediciones) y proponer alternativas antes de desviarse.

---

## 11. Cómo compilar y ejecutar

```bash
# Requisitos: Rust stable (rustup), drivers GPU con Vulkan/DX12/Metal
cargo run --release -p mv_game
cargo test --workspace
cargo bench
cargo run --release -p mv_game --features profile   # con Tracy
```

Controles (objetivo): `WASD` mover · `Espacio/Ctrl` subir/bajar · `Shift` rápido · `Click izq.` quitar · `Click der.` poner · `Rueda` radio del pincel · `1–9` material · `F3` overlay debug · `F5` vista de temperatura.

---

## 12. Estado del proyecto

> El agente actualiza esta sección al terminar cada hito.

| Hito | Estado | Notas / métricas |
|---|---|---|
| 0 — Esqueleto | ✅ Completado (2026-11-16) | Workspace de 7 crates + `benches/`. wgpu 30.0.2 / winit 0.30.13 (compatibles con egui 0.37). Ventana estable, adaptador logueado (RTX 5060 Laptop, Vulkan, `Bgra8UnormSrgb`). Tick fijo 30 Hz verificado (30 ticks/s a 60 FPS con vsync). 20 tests (coords exhaustivos incl. negativos, timestep, config). Bench: `world_to_chunk_local` 4096 conv. = 2,46 µs; roundtrip de índice de un chunk completo (262 144) = 56 µs. ADR 001 (tracing/Tracy) y 002 (convenciones del workspace). |
| 1 — Chunk raymarcheado | ✅ Completado (2026-11-16) | Chunk 64³ uniforme/denso + `BrickMask`; escena de prueba (terreno sinusoidal, agua, esferas de arena y lava). `MaterialRegistry` desde RON adelantado del hito 3 (el render necesita la tabla de colores). Brickmap en storage buffers (ADR 003), compute DDA bricks→vóxeles, HDR + tonemap ACES, cámara fly con posición `f64`. **~3 600 FPS (0,28 ms) a 1920×1080 sin vsync** en RTX 5060 Laptop (objetivo ≥144). 281 bricks subidos. 45 tests. Bench: generar chunk de prueba 4,2 ms (altura calculada por vóxel; optimizar por columna en hito 2), copiar 512 bricks 25 µs, ocupación 31 µs. |
| 2 — Mundo y edición | ✅ Completado (2026-11-16) | Chunks por bricks uniformes/densos (ADR 004). Terreno por ruido fBm (colinas, detalle, montañas *ridged* enmascaradas) con capas pasto/tierra/piedra, playas de arena y agua bajo el nivel del mar; alturas cacheadas por columna y clasificación rápida de chunks/bricks uniformes. Streaming en cilindro de radio 16 (horizontal) × 8 (vertical) chunks con rayon, histéresis de descarga y *stash* de chunks modificados. GPU: ventana de chunks que sigue a la cámara, entradas `UNIFORM_BIT` (piedra/agua/aire sin memoria de pool), DDA de 3 niveles chunk→brick→vóxel (ADR 005). Subida: ediciones sin presupuesto (mismo frame) y carga con 8 MiB/frame. Picking DDA en CPU (`f64`), pincel esférico (LMB quitar, RMB poner sin pisar sólidos, rueda radio, 1–9 material) con vista previa, overlay egui (F3). **`--bench-fly` 1920×1080 sin vsync, 5 chunks/s a 10 m del suelo: media 1,42 ms (~700 FPS), p99 3,25 ms, máx. 6,2 ms, 0 frames > 16,7 ms**; carga inicial 0,43 s. 14 586 chunks cargados (13 324 uniformes): 132 MiB CPU, 122 MiB de bricks GPU. El DDA de chunks bajó la media de 2,60 a 1,35 ms. 69 tests. Bench: chunk de superficie 100 µs (con alturas en caché), alturas de una columna 64×64 676 µs, pincel r=16 cavar+rellenar 231 µs, raycast rasante de 120 vóxeles 226 ns, ocupación de bricks 31 → 1 µs. |
| 3 — Autómata celular | ✅ Completado (2026-11-16) | `mv_sim`: polvo (cae y desliza por las 4 diagonales inferiores), líquido (cae, diagonales, se dispersa hasta `dispersion`), gas (sube, diagonales, deriva); movimientos como intercambios ordenados por densidad (arena se hunde en agua, aceite flota). RNG = hash(seed, tick, posición). Chunks activos/dormidos con AABB sucio (±1, se propaga a vecinos), sueño tras `sleep_after_ticks`, despertar por edición. Tablero 3D de 8 fases con rayon **sin `unsafe`**: cada tarea saca su chunk del mundo y lee vecinos inmutables; los movimientos entre chunks se encolan y se aplican en serie validados (ADR 006). Resultado idéntico con 1 y 4 hilos. Tierra pasa a sólido estático. Overlay: chunks activos, movimientos/tick, botón "Drop 100k", pausa (P). **Aceptación — `sim_drop` (102 400 vóxeles en cuenca 160×160): máx. 4,5 ms/tick, media 3,2 ms en los 300 primeros ticks; montículo de 45° (altura 31), superficie del agua y=2..3, masa conservada. En el juego (`--bench-drop=30`, sobre terreno, con render): media 3,61, p95 5,12, máx. 6,56 ms/tick, hasta 45 chunks activos.** 87 tests (17 de reglas en grids pequeños deterministas). Limitación: una capa de líquido incompleta no se duerme (difusión), ~1,2 ms/tick para un estanque de 51k vóxeles. |
| 4 — Temperatura | ✅ Completado (2026-11-16) | Grilla gruesa 4³ (16³ celdas/chunk) dispersa: solo chunks que se apartan del ambiente; cada celda guarda T, capacidad (Σ densidad×heat_capacity) y conductividad. Difusión Jacobi en paralelo con flujo antisimétrico limitado por la capacidad menor (conserva energía, estable); advección: los vóxeles que se mueven llevan su energía (también entre chunks); campos que crecen a vecinos y se duermen. Transiciones por vóxel con histéresis (2 °C), probabilidad 10 %/tick y calor latente (`latent_heat`, nuevo campo de material); el material colocado nace a su `base_temperature`. Vista F5 (mapa de calor en el shader) y temperatura bajo la mira en el overlay; shaders validados con naga en `cargo test` (ADR 007). **Aceptación: test con los materiales reales — lava sobre hielo funde el hielo, el agua hierve y el vapor se condensa a y ≥ 40 (≥ 25 vóxeles sobre la fuente); test de conservación en sistema cerrado con contabilidad explícita del desequilibrio de los cambios de fase (deriva < 1 % del calor inyectado).** Bench `sim_thermal` (8 192 lava + 32 768 hielo): media 2,95, p95 4,10, máx. 5,43 ms/tick, ≤ 56 chunks térmicos; en el juego (`--bench-heat`): media 2,45, máx. 4,59 ms/tick. 99 tests. Limitación: a 4³ los materiales de una celda comparten temperatura (interfaz lava-hielo muy reactiva). |
| 5 — Reacciones y fuego | ⬜ Pendiente | |
| 6 — Integridad y rígidos | ⬜ Pendiente | |
| 7 — Iluminación | ⬜ Pendiente | |
| 8 — Jugador y guardado | ⬜ Pendiente | |