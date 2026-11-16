# 005 — Streaming de chunks y ventana de render en GPU

- **Estado:** aceptada
- **Fecha:** 2026-11-16
- **Hito:** 2

## Contexto

El hito 2 pide un mundo por chunks con carga y descarga por distancia (≥ 16 chunks de
radio) sin tirones, y que la edición se vea en el mismo frame o en el siguiente.

PROMPT.md fija el qué, pero deja abiertos:
- la forma de la región cargada;
- dónde se genera;
- cómo se mueve el volumen de GPU con la cámara;
- cómo se prioriza la subida.

## Decisiones

### Región y streaming (`mv_world::stream`)

1. **La región cargada es un cilindro alrededor de la cámara.** Radio horizontal
   `render_distance_chunks` (16) y `render_height_chunks` (8) chunks arriba y abajo.
   Se pide primero lo más cercano.
2. **La generación corre en el pool de rayon.** Los resultados vuelven por un canal y se
   insertan en el hilo principal: el `World` nunca se comparte entre hilos y no hay locks
   en datos de vóxeles. El número de trabajos en vuelo está limitado
   (`max_inflight_jobs`).
3. **Histéresis al descargar.** Un chunk se descarga cuando sale de la región más
   `unload_margin_chunks` (2). Así se evita cargar y descargar repetidamente en el borde.
4. **Los chunks modificados no se pierden.** Al descargarse pasan a un *stash* en memoria
   y se restauran en vez de regenerarse. El hito 8 lo persistirá en disco.

### Terreno (`mv_world::terrain`)

5. **Alturas por columna en caché.** Las 64×64 alturas de una columna de chunks se
   calculan una vez y se guardan en un `Mutex<HashMap>` por columna. El lock se toma por
   columna, no por vóxel.
6. **Clasificación rápida.** Con el mínimo y el máximo de altura de la columna (y de cada
   columna de bricks) se decide sin tocar vóxeles si un chunk o brick es todo piedra, todo
   aire o todo agua. Un test compara el resultado con una evaluación vóxel a vóxel.

### Ventana en GPU (`mv_render::gpu_volume`)

7. **Residencia separada de la posición en la ventana.** Los chunks son *residentes* en la
   GPU (slot de chunk + bricks del pool) con independencia de la ventana de render.
   - La ventana es una caja de `(2r+1) × (2h+1) × (2r+1)` chunks centrada en la cámara.
   - Cuando la cámara cambia de chunk, solo se reconstruye `chunk_grid` desde el conjunto
     residente: 33×17×33 × 4 B ≈ 74 KiB.
   - No se mueve ni se resube ningún brick.
8. **Codificación de entradas compartida en todos los niveles:**
   - `EMPTY` = aire;
   - `UNIFORM_BIT | material` = chunk o brick de un solo material, sin más memoria;
   - si no, slot del nivel siguiente.

   La piedra enterrada y el agua profunda no ocupan pool. Esto amplía el ADR 003.
9. **DDA de tres niveles en el shader:** chunk → brick → vóxel. Un chunk vacío cuesta un
   paso en vez de hasta 24. Lo justificó la medición (punto 11).

### Subida y prioridades (`mv_game::game_world`)

10. **Orden de subida por frame:**
    1. descargas;
    2. ediciones, siempre completas y sin presupuesto (por eso se ven en el mismo frame);
    3. chunks recién cargados, hasta `upload_budget_kib` (8 MiB) por frame.

    Un chunk que obtiene slot nuevo sube sus 512 bricks, no solo los sucios. Puede haber
    sido uniforme hasta ese momento.

## Mediciones

RTX 5060 Laptop, 1920×1080, sin vsync, radio 16/8. `--bench-fly`: 20 s a 20 m/s
(5 chunks/s), 10 m sobre el suelo.

11. **DDA de dos niveles** (bricks → vóxeles): media 2,60 ms, p99 5,06 ms.
12. **DDA de tres niveles:** media 1,42 ms, p99 3,25 ms, máximo 6,2 ms.
    - 0 frames por encima de 16,7 ms.
    - Ninguna subida pendiente al final de cada frame.
    - Carga inicial completa en 0,43 s.
13. **Memoria con 14 586 chunks cargados** (13 324 uniformes):
    - 132 MiB de vóxeles en CPU;
    - 122 MiB de bricks en GPU;
    - 1 262 slots de chunk.

    Con arrays densos (spec literal) serían ~640 MiB solo por los chunks no uniformes.

## Consecuencias

- **Las descargas pierden los cambios hasta el hito 8.** El stash vive solo en memoria y
  se pierde al cerrar.
- **La simulación del hito 3** podrá reutilizar las colas `edited`/`loaded` de `World`
  para marcar chunks activos y subir sus cambios.
- **Si el pool de bricks se llena**, se avisa una vez en el log y los bricks que no caben
  no se dibujan. La ocupación aparece en el overlay (F3).
