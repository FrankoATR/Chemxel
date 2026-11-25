# 012 — Líquidos en reposo, evaporación y nubes

- **Estado:** aceptada; la regla de reposo y la evaporación de los sueltos quedan
  sustituidas por los niveles de llenado (ADR 014). El vapor y las nubes siguen igual.
- **Fecha:** 2026-11-25
- **Contexto:** petición del usuario tras el hito 8

## Contexto

Un charco o una piscina casi nunca tiene la capa de arriba completa. Con la regla de
líquidos del hito 3, cada vóxel suelto de esa capa se desplazaba a un lado en todos los
ticks buscando un hueco y nunca encontraba el equilibrio. Se veía un temblor caótico y
esos chunks no se dormían nunca: el benchmark `sim_drop` no llegaba a asentarse. Era una
limitación conocida desde ADR 006.

El usuario propuso que esos vóxeles sueltos desaparezcan como en la realidad: el agua se
evapora, sube y se queda como nube (sin lluvia por ahora), y la lava hace costra.

## Decisiones

1. **Regla de reposo.**
   - Un líquido se mueve a un lado solo si dentro de su `dispersion` encuentra un sitio
     por donde caer, o si tiene encima más del mismo líquido (presión: una columna se
     extiende).
   - Si no, reposa. Solo se mueve una celda con probabilidad 1/8 por tick, lo justo para
     que las terrazas anchas terminen de nivelarse. Sin eso, el agua quedaba en
     escalera.
   - Un vóxel que todavía puede deambular queda «vivo» (ADR 008). Cuando ninguno tiene
     sitio al lado, el chunk se duerme.
2. **Evaporación de lo suelto.**
   - Afecta a un vóxel líquido en reposo con aire encima que sea:
     - una gota o un bulto: como mucho un vecino del mismo líquido a los lados;
     - o el borde de un hueco de su capa: aire al lado con el mismo líquido debajo de
       ese aire.
   - Se convierte en `evaporates_into` con probabilidad `evaporate_rate` por tick.
   - Las capas completas no se tocan.
   - Valores:

     | Líquido | Se convierte en | Probabilidad por tick |
     |---|---|---|
     | Agua | `vapor` | 0,02 |
     | Lava | `basalt` (costra oscura) | 0,004 (antes 0,03; ver «Lava lenta») |
     | Ácido | aire | 0,01 |
3. **Vapor que condensa en la capa de nubes.**
   - **Primera versión:** el vapor viajaba vóxel a vóxel hasta la altura de nubes (480,
     30 m). Miles de vóxeles sueltos cruzando ~400 vóxeles de aire despertaban ~100
     chunks con regiones enormes, y el juego pasaba de 3,7 a 6,3–6,7 ms/tick de media.
   - **Ahora:** el vapor (`spreads: false`, sube 2 celdas por tick) vive solo 15–45
     ticks. Se ve salir del charco y, al terminar, deja un vóxel de `cloud` en la
     columna, a su `ceiling`, o en el primer hueco por debajo (la nube engorda hacia
     abajo). Si el vapor llega a esa altura por sí mismo, se convierte allí. Las nubes
     son más ligeras que el vapor, así que este no las atraviesa.
4. **Nubes quietas que se desvanecen sin despertar la simulación.**
   - `cloud` tiene `drifts: false` y un `lifetime` de 3 000–9 000 ticks.
   - Los gases quietos con vida no marcan su chunk como vivo. Una pasada lenta cada 300
     ticks recorre solo los chunks que tienen nubes y aplica la probabilidad equivalente
     a esos 300 ticks. Vida media de unos 2–3 minutos.
5. **Gases con `dispersion`:** suben hasta `dispersion` celdas por tick (antes, siempre
   una). Los demás gases siguen con 1.
6. **Campos de material nuevos:** `evaporates_into`, `evaporate_rate`, `ceiling`,
   `ceiling_into`, `drifts` y `spreads`. Los materiales nuevos (`vapor`, `cloud`,
   `basalt`) van al final del archivo, para que los ids existentes no cambien.

## Lava lenta (revisión)

Petición del usuario: la lava se solidificaba demasiado rápido (un charco de 1 m × 25 cm
sobre piedra quedaba con un 4 % fundido a los 10 s).

- **Causa principal:** la piedra del suelo comparte las celdas térmicas de 4³ con la
  lava y le roba el calor en pocos ticks. Bajar solo las probabilidades de
  solidificación y de costra no bastaba: la lava ya estaba fría y solo se retrasaba
  el cambio unos segundos.
- **Campo nuevo `transition_rate`** (por material, 0–1): probabilidad por tick de sus
  cambios de fase una vez pasado el umbral. Si falta, se usa `sim.transition_rate`.
- **Lava:** `heat_capacity` 3,0 y `latent_heat` 3·10⁶ (más masa térmica), conductividad
  0,8, sigue líquida hasta 300 °C (`freeze_at`), `transition_rate` 0,004 y costra
  (`evaporate_rate`) 0,004.
- **Resultado** (mismo charco): fundido el 84 % a los 10 s, el 62 % a los 20 s, el 34 %
  a los 40 s, el 14 % al minuto y sólido del todo hacia los 90 s.
- **Efecto secundario:** al llevar más energía, la lava calienta más lo que la rodea.
- **Coste:** `sim_thermal` media 3,4 ms, máx. 4,6; `--bench-heat` media 2,7 ms, p99 4,7,
  máx. 6,4 (0 ticks > 8 ms).

## Mediciones (enchufado)

| Escenario | Antes | Ahora |
|---|---|---|
| `sim_drop` (100k de arena y agua, sin ventana) | media 1,7 ms, máx. 4,6; **nunca se asentaba** | media 2,0 ms, máx. 5,9; **se asienta del todo a los 2 806 ticks** (93 s) |
| `--bench-drop` (en el juego, ladera real) | media 3,7 ms, máx. 6,5; el agua temblaba sin parar | media 4,35 ms, p99 6,95, máx. 8,37 (2 de 1 801 ticks > 8 ms); el agua acaba quieta |
| `sim_thermal` | máx. 5,3 ms | máx. 3,8 ms |
| `sim_fire` | máx. 6,9 ms | máx. 7,0 ms |

En la ladera, la película de agua y las gotas sueltas desaparecen en unos segundos, y el
agua que queda se junta en los canales entre terrazas.

## Tests

- **Capa incompleta:** una piscina con la capa de arriba incompleta pierde esa capa,
  conserva las completas y luego no se mueve.
- **Piscina llena:** no pierde agua.
- **Gota suelta:** se convierte en su producto.
- **Vapor en el techo:** se hace nube quieta.
- **Vapor que muere:** deja nube en su techo, la nube no despierta la simulación y se
  desvanece.
- **Nube que engorda:** crece hacia abajo.
- **Regresión:** el test de nivelado del hito 3 sigue pasando.

## Limitaciones conocidas

- **Pérdida de agua:** las películas finas sobre laderas se evaporan, así que se pierde
  más agua que antes (es el precio de que no tiemblen).
- **Las nubes no se mueven ni llueven.** Si se juntan muchas, la siguiente evaporación
  bajo ellas se pierde cuando no hay sitio en 24 vóxeles.
- **Nubes fuera de la zona cargada:** la nube se coloca solo si esa altura está cargada.
- **Sin humo de la lava:** la costra de lava no suelta humo; quedaría para más adelante.
