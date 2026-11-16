# 007 — Temperatura en grilla gruesa y transiciones de fase

- **Estado:** aceptada
- **Fecha:** 2026-11-16
- **Hito:** 4

## Contexto

PROMPT.md §6.3–6.4 pide:
- un campo de temperatura (grilla gruesa de 4³ para empezar);
- difusión explícita con conductividad por material, ponderada por la capacidad
  calorífica y estable (CFL);
- fuentes de calor;
- transiciones de fase con calor latente opcional.

También un test de conservación aproximada de la energía.

## Decisiones

1. **Grilla gruesa y dispersa.** Solo los chunks que se apartan de la temperatura
   ambiente (`ambient_temperature`, 20 °C) tienen campo: 16³ celdas de 4³ vóxeles. El
   resto está a temperatura ambiente sin coste.
   - Cada celda guarda la temperatura, la capacidad (suma de
     `densidad × heat_capacity` de sus vóxeles) y la conductividad media.
   - Una celda de 4³ cae siempre dentro de un brick de 8³: un brick uniforme da la
     composición de 8 celdas de golpe.
2. **Difusión explícita tipo Jacobi y en paralelo.** Cada chunk lee las temperaturas
   viejas de todos y escribe un búfer propio.
   - Flujo por cara: `g·(Tb − Ta)`, con `g = diffusion_rate × media armónica(ka, kb)`.
   - `g` se limita a `0,9 · min(Ca, Cb) / 6`. Así el paso es estable con cualquier
     contraste de capacidades (aire contra lava).
   - El flujo es antisimétrico, de modo que conserva la energía exactamente.
3. **Advección.** Cuando el autómata mueve un vóxel a otra celda, se lleva su energía
   `c·T` y la celda de destino mezcla. Funciona también entre chunks: la cola de
   movimientos lleva la temperatura. Así el vapor transporta calor y la lava que fluye
   calienta lo que toca.
4. **Crecimiento y sueño.**
   - Un vecino recibe campo cuando la cara compartida se aparta más de
     `thermal_epsilon` (0,5 °C).
   - Un campo que vuelve a temperatura ambiente se duerme tras `sleep_after_ticks`.
   - También recibe campo el chunk al que llega material inestable a temperatura
     ambiente (vapor) o un vóxel caliente.
5. **Fuentes de calor = temperatura de creación.** El material colocado nace a su
   `base_temperature` (lava a 1300 °C, hielo a −10 °C) mediante `Simulation::edited`. No
   hay fuentes infinitas: la lava se enfría y solidifica. El fuego (hito 5) será la
   primera fuente activa.
6. **Transiciones por vóxel.**
   - Se disparan pasado el umbral más `transition_hysteresis` (2 °C), con probabilidad
     `transition_rate` (10 %) por tick. Esto evita que una celda entera cambie de golpe y
     que oscile en el umbral.
   - El vóxel que cambia absorbe o libera `latent_heat(B) − latent_heat(A)`. Es un campo
     nuevo del material, en unidades de juego.
7. **Conservación con contabilidad explícita.** Un vóxel conserva su volumen al cambiar de
   fase, así que no se pueden conservar a la vez masa y energía: un vóxel de vapor
   representaría 1 600 veces menos masa que uno de agua.
   - Se conserva la temperatura de la celda, salvo el calor latente.
   - La diferencia de energía sensible se acumula en `energy_imbalance`, igual que lo que
     se pierde al dormir campos.
   - El test de sistema cerrado comprueba que `E_final − E_inicial − imbalance ≈ 0`.
8. **Vista de depuración (F5).** Los campos se empaquetan cada frame en un pool de GPU
   (`heat_index` por celda de chunk de la ventana + `heat_pool`). El shader colorea los
   impactos con una rampa: azul (frío), gris (ambiente), amarillo (100 °C), naranja y
   blanco.
9. **Validación de shaders en tests.** Los WGSL se parsean y validan con `naga`
   (reexportado por wgpu) en `cargo test`.

## Mediciones (RTX 5060 Laptop / 24 hilos)

| Escenario | Media | p95 | Máximo | Chunks térmicos (máx.) |
|---|---|---|---|---|
| `sim_thermal`: 8 192 vóxeles de lava sobre 32 768 de hielo, cuenca de piedra, 900 ticks | 2,95 ms/tick | 4,10 | 5,43 | 56 |
| `--bench-heat`: lava sobre hielo en el juego real, con render | 2,45 ms/tick | 3,44 | 4,59 | 38 |

- **Conservación del agua:** hielo + agua + vapor se mantiene en 32 768 vóxeles.
- **Test de aceptación:** el hielo se funde, el agua hierve, el vapor sube y aparece agua
  condensada a más de 25 vóxeles por encima de la fuente (y ≥ 40).

## Limitaciones conocidas

- **Resolución de 4³.** Los materiales que comparten celda comparten temperatura. La
  interfaz lava-hielo reacciona de forma violenta: la lava que entra en una celda de hielo
  se mezcla con ella al instante. Si molesta, la alternativa es temperatura por vóxel en
  las celdas mixtas.
- **`lifetime` de los gases.** Sigue sin aplicarse: el vapor desaparece al condensarse.
- **Persistencia.** Al descargar un chunk su campo se pierde y vuelve a temperatura
  ambiente. La persistencia es del hito 8.
