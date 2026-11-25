# 014 — Líquidos con nivel de llenado

- **Estado:** aceptada (sustituye la regla de reposo y la evaporación de sueltos de
  ADR 012)
- **Fecha:** 2026-11-25
- **Contexto:** petición del usuario («agua v2», para todos los fluidos)

## Contexto

Un vóxel era agua o no lo era. Una piscina casi nunca tenía la capa de arriba completa, y
esos vóxeles sueltos temblaban. ADR 012 los evaporaba para que el agua se calmara, pero
se perdía agua y la superficie seguía siendo una escalera de vóxeles. El usuario quería
un agua como la de las demos de vóxeles con agua fluida: superficie lisa, que se derrame
y forme ríos.

## Decisiones

1. **Nivel de llenado por vóxel líquido**, de 1 a `LEVELS` = 255 (lleno).
   - **Almacenamiento:** aparte en el `Chunk`: una tabla de 512 bricks con `[u8; 512]`
     solo donde hay algún vóxel parcial. Sin nivel guardado, el vóxel está lleno, así
     que el terreno, los sólidos y los gases no pagan nada.
   - **Bricks densos:** un vóxel parcial obliga a que su brick sea denso, y `compact`
     no lo compacta. Así un brick o chunk uniforme siempre está lleno, y el código que
     lo supone sigue siendo correcto.
   - **Por qué 255 niveles:** con 16, la regla de reparto podía dejar una pendiente
     estable de 1/16 de vóxel por celda (un lago inclinado).
2. **Reglas** (`automaton::liquid`), iguales para todos los líquidos:
   1. **Cae** entero, con su nivel, o resbala por una diagonal (como antes).
   2. **Se vierte** en el mismo líquido a medio llenar que tenga debajo, y sigue con
      el resto. Sin esto, el borde de un montón solo goteaba hacia la capa de abajo y
      tardaba miles de ticks en extenderse.
   3. **Reparte** con sus 4 vecinos:
      - con el mismo líquido, la mitad de la diferencia. Los vóxeles parciales (la
        capa de arriba) comparan con el más bajo hasta `dispersion` celdas por la
        misma superficie, y así un lago se nivela ~`dispersion`² veces más rápido;
      - al aire, por un borde: la mitad, o todo si es poco. Por debajo de `POUR_MIN`
        (16/255) la película se queda pegada, porque si no las laderas rugosas
        goteaban sin parar;
      - al aire, sobre su propio líquido (la capa de arriba), libremente;
      - una lámina sobre una capa llena se vierte directamente en una celda parcial
        de esa capa a hasta `dispersion` celdas;
      - al aire, sobre el suelo, solo con al menos `min_split`: medio vóxel en el agua
        y ¾ en los viscosos. Es la «tensión superficial»: un charco deja de
        extenderse en vez de cubrirlo todo con una película.
   4. **No reparte** por debajo de `SHARE_MIN` = 8 niveles de diferencia (3 % de un
      vóxel, menos de lo que se dibuja). Si no, una cola infinita de flujos invisibles
      mantenía despiertos los lagos grandes.
   5. **Viscosidad** (`dispersion` < 4): reparte solo `dispersion`/4 de los ticks. La
      lava (1) se arrastra.
   6. **Evaporación:** solo de las películas finas sobre el suelo (no de la capa de
      arriba de un estanque). Ya no se pierde agua de las piscinas.
3. **Conservación:**
   - La cantidad (suma de niveles) se conserva en movimientos y repartos, también
     entre chunks.
   - El `Move` lleva los niveles esperados y los nuevos, y se descarta si cambiaron.
     Tras un reparto hacia otro chunk, el vóxel no reparte más en ese tick: si no, el
     movimiento se invalidaba y se perdía agua.
   - **Cambios de un vóxel parcial** (reacción, cambio de fase, evaporación):
     `partial_change`. Si el producto es otro líquido, conserva el nivel; si es otra
     cosa, aparece con probabilidad nivel/255 y si no queda aire. Así se conserva en
     promedio: un vóxel de agua casi vacío ya no hierve en un vóxel entero de vapor,
     que al condensarse daba agua de más.
4. **Energía:** la capacidad calorífica y el calor latente de un vóxel no dependen de su
   nivel. La energía que crea un vóxel nuevo en el aire (o destruye una fuente que se
   vacía) va a `energy_imbalance`, como los demás cambios no conservativos (ADR 007).
5. **Guardado:** formato `CXCH` versión 2, con una sección de niveles por brick (RLE).
   Los archivos de la versión 1 se leen como llenos.
6. **Render:**
   - **Datos:** el nivel viaja en los 4 bits altos de cada vóxel del brick pool (16
     alturas visibles); el material queda en 12 bits.
   - **Corte:** `trace_brick` corta el vóxel parcial a su altura. Un rayo que entra por
     encima choca con el plano de la superficie, o pasa por encima.
   - **Superficie lisa:** la normal sale de las alturas de los vecinos
     (`liquid_normal`), así que la superficie se ve como una sola lámina.
7. **Qué fluidos:** todos los líquidos (agua, lava, ácido) usan esta lógica. No se aplica
   a los gases (no forman superficie y suben a su techo) ni a los polvos (la arena se
   apila con su ángulo de reposo, no se nivela).

## Mediciones (enchufado, RTX 5060 Laptop, 24 hilos)

| Escenario | Antes (ADR 012/013) | Ahora |
|---|---|---|
| `sim_drop` (cuenca de 160×160, sin ventana) | media 2,0 ms, máx. 5,9; se asienta a los 2 806 ticks | media 4,7 ms, máx. 9,7; **se asienta a los 2 045 ticks, con toda la superficie a la misma altura** |
| `--bench-drop` (en el juego) | media 4,4–4,8 ms, máx. 6,9–8,4 | media 6,5 ms, p99 8,6–9,2, máx. 9,0–12,0 (50–99 de 1 200 ticks > 8 ms) |
| `sim_thermal` (lava sobre hielo) | media 3,4 ms, máx. 4,6 | media 4,3 ms, máx. 6,4–9,8 |
| `--bench-heat` | media 2,7 ms, máx. 6,4 | media 4,2–5,2 ms, máx. 7,5–10,1 |
| `sim_fire` | máx. 5,9 ms | media 1,6 ms, máx. 5,6 |

**Por qué cuesta más:** el agua se mueve de verdad. Se extiende por el terreno en
láminas (más vóxeles parciales que simular) y hay un 37 % más de movimientos por tick.
Antes, los montones de agua se quedaban quietos o se evaporaban. Al acabar todo se
duerme, y antes que con la regla anterior.

**Probado y descartado:**

- **Alcance largo para todos los vóxeles:** media de 6,4 ms en `sim_drop`.
- **Alcance 1 para todos:** no se asienta.
- **No mover los vóxeles llenos sumergidos:** no se asienta.

## Tests

- **Chunk:** los niveles densifican el brick, sobreviven a `compact` y una escritura
  llena los borra.
- **Guardado:** los niveles se guardan y se recuperan, y los archivos v1 cargan llenos.
- **Cuenca:** el agua conserva la cantidad exacta y su superficie queda plana a menos
  de 0,15 vóxeles.
- **Entre chunks:** la cantidad se conserva al cruzar bordes de chunk.
- **Capa incompleta:** se extiende en una capa parcial plana, sin evaporarse, y el
  estanque se queda quieto.
- **Película fina:** se evapora (de un vóxel de líquido sale como un vóxel de
  costra).
- **Energía:** los tests de conservación siguen pasando con los repartos.

## Limitaciones conocidas

- **Coste:** la simulación del agua cuesta más (ver la tabla). Con 100k vóxeles de
  agua sobre terreno real, un 4–8 % de los ticks pasan de 8 ms.
- **Pendiente residual:** de como mucho `SHARE_MIN` niveles entre vóxeles que se
  comparan (~1 cm).
- **Sin vasos comunicantes:** el agua no sube por un tubo en U.
- **Sin partículas:** las cascadas son vóxeles que caen; las salpicaduras y la
  espuma quedan para el siguiente paso.
- **Hielo de agua parcial:** al congelarse, un vóxel parcial se vuelve hielo entero
  o aire, con la probabilidad de su nivel.
