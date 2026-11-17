# 008 — Reacciones químicas, vida de los vóxeles y combustión

- **Estado:** aceptada
- **Fecha:** 2026-11-17
- **Hito:** 5

## Contexto

PROMPT.md §6.5 y el hito 5 piden:
- una tabla de reacciones en `assets/reactions/*.ron` con búsqueda O(1), probabilidad por
  tick, rango de temperatura, productos y calor;
- fuego que se propague por la madera, humo, ceniza y ácido;
- que el agua y la lava formen obsidiana en la interfaz;
- recarga en caliente de los RON en debug.

La aceptación es que un bosque arda de forma creíble.

## Decisiones

1. **Reacciones dirigidas y evaluadas en el barrido del autómata.**
   - `a + b → result_a + result_b` se evalúa al recorrer un vóxel de `a` que tiene a `b`
     como vecino de cara. Se prueban las 6 caras empezando por una aleatoria.
   - Cada par `(a, b)` se define una sola vez, así que la probabilidad es «por tick y
     contacto» sin contar doble.
   - Búsqueda: matriz densa `a × b` de `u16`, con 0 = sin reacción.
   - Ir dentro del barrido reutiliza lo que ya había: las 8 fases en paralelo, la vista
     3×3×3, la cola de cambios entre chunks validada en serie (ahora `Move` también lleva
     reacciones) y la región sucia.
   - El vóxel que reacciona queda marcado como movido en este tick.
2. **Energía de las reacciones.**
   - Los productos absorben o liberan la diferencia de `latent_heat`, igual que un cambio
     de fase: la lava que se apaga en obsidiana libera su calor latente y hierve el agua
     de alrededor.
   - `heat` es energía extra y va a la celda de `b`, lo que es tocado: una llama calienta
     lo que toca.
   - El calor liberado nunca sube la celda por encima de la `base_temperature` de `a`:
     una llama a 800 °C no calienta a 4 000 °C una celda casi vacía.
   - Todo se suma a `energy_imbalance` (ADR 007).
3. **Saltar la búsqueda cuando no hay con quién reaccionar.**
   - Cada chunk mantiene una máscara de 64 bits de los materiales que contiene (bit
     `id % 64`). Es un superconjunto: `set` añade el bit y `compact` la recalcula exacta.
   - Cada material guarda la máscara de sus compañeros de reacción.
   - Un vóxel solo mira a sus vecinos si algún compañero está en los 27 chunks de la
     vista.
   - Sin esto, el agua (que reacciona con la lava) costaba un 40 % más en `sim_drop`
     aunque no hubiera lava en ningún sitio.
4. **`lifetime` sin estado.**
   - No se guarda la edad de cada vóxel: el chunk sigue en 2 bytes por vóxel.
   - Cada tick el vóxel expira con probabilidad `2 / (min + max)` y se convierte en
     `decays_into` (por defecto aire). La vida es exponencial con media `(min + max) / 2`.
   - Determinista: hash de seed, tick y posición.
5. **Vóxeles «vivos».** Un vóxel que todavía puede reaccionar (tiene un compañero al lado
   pero la tirada falló) o caducar no ha cambiado, pero vuelve a la región pendiente
   (`TaskOut::live`). Así su chunk no se duerme y el ácido quieto sobre la piedra sigue
   comiéndosela.
6. **Combustión en el pase térmico.**
   - Un material `flammable` arde mientras su celda supera `ignite_at`. Cada tick, con
     probabilidad `burn_rate`, se convierte en `burns_into` y libera su `latent_heat`
     (energía de combustión), sin superar la temperatura de su `flame` (la
     `base_temperature` del fuego, 800 °C).
   - Si no se consume, con probabilidad `flame_rate` emite una llama en un vóxel de aire
     vecino del mismo chunk: arriba si puede, si no a un lado.
   - El fuego (`Energy`) se mueve como un gas (sube y titila), calienta lo que toca con
     reacciones (`fire + wood`, `fire + leaves`…) y caduca en humo, y el humo en aire.
   - Las hojas arden en ceniza: la ceniza caliente cae por el tronco y lo enciende desde
     abajo (advección), así que los árboles arden enteros y no solo la copa.
7. **Pérdida de calor al entorno (`heat_loss`).**
   - Sin pérdidas, el calor de un incendio se difundía por el aire sin límite: en 3D, una
     fuente constante calienta un radio enorme y los campos térmicos no se dormían nunca.
   - Cada material tiene `heat_loss` (por vóxel, tick y grado sobre el ambiente); solo el
     aire y el humo lo usan. La celda suma la de sus vóxeles y pierde
     `min(loss, 0,1·C)·(T − ambiente)` en la difusión, que sigue siendo estable.
   - Modela la convección que la grilla no resuelve. Los tests del hito 4 (condensación
     lejos de la fuente, conservación con contabilidad) siguen pasando.
8. **Recarga en caliente.**
   - En builds de debug, o con `--hot-reload`, el juego sondea dos veces por segundo la
     fecha y el tamaño de los `*.ron` de `materials/` y `reactions/`. No hace falta una
     dependencia para vigilar archivos.
   - La recarga se rechaza (con un error en el log) si los archivos no son válidos o si
     un material existente cambiaría de id: los vóxeles del mundo guardan ids.
   - Si se acepta, se reemplazan las tablas de la simulación, los campos térmicos
     recalculan su composición y se actualizan la paleta de la GPU y la lista del pincel.

## Mediciones (RTX 5060 Laptop / 24 hilos, enchufado)

| Escenario | Media | p95 | p99 | Máximo |
|---|---|---|---|---|
| `sim_fire`: bosque 6×6 (57 703 vóxeles de combustible) sobre piedra, 1 800 ticks | 1,62 ms/tick | 4,36 | — | 5,87 |
| `--bench-fire`: el mismo bosque en el terreno real, con render | 3,23 ms/tick | 5,78 | 6,82 | 9,24 (6 de 1 200 ticks > 8 ms) |
| `sim_drop` (regresión del hito 3) | 1,80 ms/tick | 3,19 | — | 4,60 |
| `--bench-drop` | 3,42 ms/tick | 4,99 | 5,64 | 6,58 |
| `--bench-heat` (ahora con obsidiana) | 2,53 ms/tick | 3,77 | 4,55 | 6,77 |

- **Bosque:** arde entero en unos 800 ticks (27 s) y no queda combustible. Picos por tick:
  ~380 llamas, ~950 reacciones y ~30 000 movimientos (sobre todo humo).
- **Batería:** desenchufado, el portátil reduce la CPU y los mismos benchmarks tardan
  ×2–2,5. Hay que medir siempre enchufado.

## Limitaciones conocidas

- **Paralelismo por chunk.**
  - El bosque cabe en 2×2 columnas de chunks: con ~12 chunks activos repartidos en
    8 fases, el autómata es casi secuencial. Es el ~80 % del tick en el pico.
  - Si hace falta más margen hay dos opciones: paralelizar dentro del chunk (subbloques
    32³ con su propio tablero de ajedrez) o el LOD temporal de PROMPT.md §6.8.
- **Humo opaco.** El render todavía no tiene translucidez (hito 7): el humo se ve como
  vóxeles grises sólidos. Se acortó su vida para que no tape la escena.
- **Probabilidad de las reacciones.** La reacción se evalúa desde `a`. Si `a` es un sólido
  estático en un brick uniforme, nunca se recorre: las reacciones deben empezar en
  materiales que se mueven. Está documentado en el README del crate.
- **Agua dentro de la lava.** La lava intercambia sitio con burbujas de vapor y el agua se
  cuela en esos huecos, así que a veces se forma obsidiana dentro del charco y no solo en
  la interfaz. Es emergente y verosímil; el test exige que al menos el 75 % esté en la
  interfaz.
- **Llamas entre chunks.** Las llamas solo se emiten dentro del chunk del vóxel que arde.
  En el borde se pierden algunas, sin efecto visible.
