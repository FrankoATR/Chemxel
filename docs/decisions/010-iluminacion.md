# 010 — Iluminación en el raymarcher: sombras, AO, emisivos y transparencia

- **Estado:** aceptada
- **Fecha:** 2026-11-18
- **Hito:** 7

## Contexto

PROMPT.md §7.3 y el hito 7 piden:
- sombras del sol con un rayo secundario;
- AO por rayos cortos;
- emisión (la lava y el fuego brillan);
- transparencia simple para agua y vapor;
- niebla.

**Aceptación:** la escena debe verse «tipo Teardown» manteniendo ≥ 60 FPS. Todo vive en el
compute shader `raymarch.wgsl`, que ya tenía el DDA de 3 niveles y los cuerpos rígidos.

## Decisiones

1. **El DDA admite `skip` y `t_limit`.** `trace(ro, rd, skip, t_limit)` trata los vóxeles
   del material `skip` como vacíos y para tras `t_limit`. Sirve igual para el rayo
   primario, el rayo de sombra y el rayo que mira a través de un material translúcido.
2. **Sombras del sol.**
   - Desde el impacto (desplazado sobre la normal), un `trace` hacia el sol limitado a
     `render.shadow_distance` (768 vóxeles = 48 m), más los cuerpos rígidos.
   - Un material translúcido deja pasar parte de la luz y el rayo sigue una vez más allá
     de él.
3. **AO a dos escalas.**
   - **Esquinas:** ocupación de los 8 vóxeles alrededor de la cara impactada,
     interpolada por esquina con la posición dentro de la cara (estilo «AO de vértice»).
     Es barato, nítido y sin ruido.
   - **Rayos cortos:** 5 direcciones en cono alrededor de la normal, muestreadas a 2,5, 5,
     9 y 15 vóxeles con `voxel_at`. Unas 20 consultas por píxel, sin ruido ni
     acumulación temporal.
   - `render.ao_strength` (0,8) escala el resultado; 0 lo apaga.
4. **Emisivos.**
   - Los materiales con `emissive > 0` no reciben sombra ni AO y escriben más de 1 en el
     búfer HDR.
   - Las muestras de AO que caen en un vóxel emisivo suman su luz a la superficie: la
     lava y el fuego iluminan lo que tienen cerca, sin un sistema de luces.
   - Un *bloom* barato en el blit (dos anillos de 8 muestras del exceso sobre 1) les da
     halo.
5. **Transparencia de una capa.**
   - Cuenta como translúcido un material con alfa < 0,98: agua, vapor, hielo, vidrio,
     humo, ácido.
   - El color de detrás sale de otro `trace` que ignora ese material.
   - La absorción (Beer-Lambert, el alfa es la opacidad de ~8 vóxeles) usa solo el grosor
     recorrido dentro del material, medido con un DDA corto de vóxeles contiguos. Sin
     esto, una gota aislada contra el cielo salía azul opaca.
   - Los líquidos (nuevo `MaterialGpu.params.y`) reflejan el cielo con Fresnel (Schlick).
6. **Cielo y niebla.**
   - Disco y halo del sol en el cielo.
   - La niebla por distancia empieza a la mitad de la distancia de render (antes al
     35 %) y toma el color del cielo, cálido hacia el sol.
   - Más variación de brillo por vóxel (±9 %) para dar textura, y un césped menos
     saturado.
7. **Calidad configurable.**
   - `render.sun_direction`, `shadow_distance` y `ao_strength` en `config.ron`.
   - En el juego, F6 alterna entre tres presets: completo, solo sombras y sin
     iluminación.
   - `--lighting=full|shadows|off` arranca con uno, para comparar con `--bench-fly`.

## Mediciones (RTX 5060 Laptop, 1080p, sin vsync, enchufado)

`--bench-fly` (20 s volando a 20 m/s sobre el terreno, con streaming):

| Preset | Media | p95 | p99 | FPS medios |
|---|---|---|---|---|
| Sin iluminación (`--lighting=off`) | 2,12 ms | 3,05 | 4,68 | ~470 |
| Solo sombras | 3,06 ms | 4,16 | 4,65 | ~330 |
| Completo | 4,05–4,19 ms | 5,0–5,4 | 5,5–7,4 | ~240 |

**Escenas:**

| Escena | FPS (overlay) |
|---|---|
| Incendio | ~190–200 |
| Lava sobre hielo con vapor y agua translúcida | ~196 |
| Torre cayendo con su sombra | ~140–170 |

La aceptación (≥ 60 FPS) se cumple con margen de ×3–4.

## Revisión: vóxeles que no parezcan cubos grandes

Petición del usuario tras el hito 8: los vóxeles seguían leyéndose como cubos grandes.
Había dos causas: el ruido de brillo por vóxel (±9 %) marcaba cada vóxel como una
casilla distinta, y el sombreado con la normal de la cara convertía cada escalón de un
vóxel en una línea dura.

1. **Normales suavizadas** (`render.smooth_normals`, 0–1, por defecto 0,5).
   - Para la cara que se ve, `smooth_normal` mide la altura de la superficie a 1, 2 y 3
     vóxeles en las cuatro direcciones de la cara: +1 si sube, −1 si baja, 0 si sigue
     plana.
   - Esa pendiente, ponderada por 1/distancia, inclina la normal de la cara. Una
     escalera de 45° se sombrea casi como un plano a 45°, y las aristas quedan
     biseladas.
   - Los materiales translúcidos solo cuentan consigo mismos: el agua sobre piedra tiene
     su propia superficie.
   - La normal suavizada se usa para el sol y la luz del cielo. La cara sigue mandando
     en el origen de los rayos (sombra y AO), para no meterlos en el vóxel vecino. Si
     la cara mira al lado contrario del sol pero la normal suavizada no, el rayo de
     sombra sale del vóxel de aire de delante.
   - Primer intento descartado: el gradiente de ocupación del entorno 3³. Biselaba las
     terrazas, pero en las pendientes suaves cada escalón quedaba como una «escama»
     clara y oscura, peor que antes.
2. **Menos ruido de color.**
   - El ruido por vóxel queda en el 30 % (`render.color_noise`; 1 = el aspecto
     anterior).
   - Se añaden manchas amplias de ruido de valor suave, de unos 6 vóxeles, para que la
     superficie no quede plana.

**Coste** (`--bench-fly`, enchufado, 1600×900): media 2,6–2,8 ms con el aspecto
anterior frente a 3,1–3,2 ms ahora (unos 320 FPS). Con `smooth_normals: 0` se ahorran
las 24 lecturas extra por píxel.

**Limitación:** en pendientes muy suaves (un escalón cada más de 6 vóxeles), entre
escalones la superficie queda plana y se ven bandas suaves a lo largo de las curvas de
nivel. Haría falta una ventana más grande, con más lecturas por píxel.

## Limitaciones conocidas

- **Una sola capa translúcida:** el vapor delante de agua muestra el agua opaca.
  Tampoco hay refracción ni escena bajo el agua vista desde dentro.
- **AO aproximada:** con muestras fijas, sin ruido, los huecos de menos de 2 vóxeles no
  oscurecen del todo. La luz de los emisivos llega hasta ~15 vóxeles y no proyecta
  sombras.
- **Sombras duras:** sin penumbra. Más allá de `shadow_distance` no hay sombra.
- **Sin GI:** el rebote de luz con acumulación temporal queda como «Futuro» en
  PROMPT.md §7.3.
