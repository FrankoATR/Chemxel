# 006 — Autómata celular: fases en paralelo sin `unsafe`

- **Estado:** aceptada
- **Fecha:** 2026-11-16
- **Hito:** 3

## Contexto

PROMPT.md §6.2 y §6.7 piden:
- un autómata celular (polvos, líquidos, gases);
- solo chunks activos y, dentro de ellos, una región sucia (AABB);
- paralelismo con un tablero de ajedrez 3D de 8 fases (2×2×2);
- aleatoriedad determinista por seed + tick + posición.

El problema: dos chunks de la misma paridad (separados por 2) comparten vecinos. Si cada
tarea escribe directamente en sus vecinos, dos tareas pueden escribir en el mismo chunk a
la vez. En Rust eso exige `unsafe` o bloqueos por vóxel.

## Decisiones

1. **Tarea = un chunk; escritura solo en el propio chunk.**
   - En cada fase, sus chunks se sacan del `World` (`World::detach`) y se procesan en
     paralelo con rayon.
   - Cada tarea lee sus 26 vecinos a través de `&World`. Ningún vecino se muta durante
     esa fase: cualquier vecino difiere en paridad en al menos un eje.
   - Rust seguro, sin locks.
2. **Movimientos entre chunks diferidos.**
   - Un movimiento cuyo destino está en un vecino se encola como `Move`: origen, destino
     y los dos materiales esperados.
   - Al acabar la fase se aplica en serie y en orden determinista (posición de chunk,
     luego orden de barrido), solo si ambas celdas siguen igual.
   - Si dos tareas apuntan a la misma celda, gana la primera y la segunda se descarta.
3. **"Movido este tick" como bitset temporal por chunk activo** (32 KiB, reciclado), no
   como canal persistente de flags. Los movimientos diferidos marcan el bit en el chunk
   de destino.
4. **Región sucia y sueño.**
   - Toda celda que cambia añade su entorno ±1 a la región *pendiente* de cada chunk que
     toca, incluidos los vecinos. Así se despiertan solos.
   - Un chunk sin cambios repasa su última región durante `sleep_after_ticks` (2) ticks y
     luego se duerme; al dormirse se compacta.
   - Las ediciones del jugador despiertan con `Simulation::wake_box`.
5. **Todo movimiento es un intercambio ordenado por densidad.**
   - Los polvos y líquidos se hunden en fluidos más ligeros; los gases suben a través de
     fluidos más densos.
   - De lado solo se entra en gases, y un gas solo se expande hacia el aire.
   - Los vecinos no cargados actúan como pared.
6. **Barrido de abajo arriba**, alternando el sentido en x y z cada tick. Una columna que
   cae se mueve entera en un tick.
7. **Determinismo.** El hash depende de seed, tick y posición, sin estado de RNG. El
   resultado es idéntico con 1 o 4 hilos (hay un test).
8. **La tierra pasa a `Solid`.** Como polvo, la capa de tierra del terreno se derrumbaría
   en cada agujero. Los bloques sueltos son del hito 6.

## Mediciones

`cargo bench -p mv_benches --bench sim_drop`: 102 400 vóxeles de arena y agua en una
cuenca de piedra de 160×160, 24 hilos.

| Métrica | Valor |
|---|---|
| Tiempo por tick | máximo 4,5 ms; media 3,2 ms en los primeros 300 ticks |
| Conservación de materiales | sí |
| Montículo de arena | altura 31 (montón de 45° con ese volumen) |
| Superficie del agua | entre y=2 y y=3 |

`--bench-drop=30`, el mismo volumen sobre el terreno real con el render activo:

| Métrica | Valor |
|---|---|
| Media | 3,61 ms/tick |
| p95 | 5,12 ms/tick |
| Máximo | 6,56 ms/tick |
| Chunks activos (máx.) | 45 |

## Limitaciones conocidas

- **Una capa de líquido incompleta no se duerme.** Sus vóxeles siguen desplazándose al
  azar, porque así se nivela la superficie por difusión. La cuenca del benchmark sigue
  activa con ~1,2 ms/tick.
  - Moverse de lado solo hacia un desnivel la dormiría, pero deja pirámides escalonadas
    sin nivelar.
  - Una solución por presión o caudal queda para más adelante, si el coste importa.
- **Poco paralelismo con pocos chunks activos.** El paralelismo es por chunk: 16 chunks
  en 8 fases son ~2 tareas por fase. Hoy el coste por vóxel (~30 ns por movimiento) basta
  para cumplir el presupuesto. Si hiciera falta más, subdividir el trabajo dentro del
  chunk requeriría otro reparto de la memoria.
- **El `lifetime` de los gases** (vapor, humo) aún no se aplica. Necesita un canal por
  vóxel y llegará con temperatura y reacciones (hitos 4–5).
