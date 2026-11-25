# 013 — Paisaje natural

- **Estado:** aceptada
- **Fecha:** 2026-11-25
- **Contexto:** petición del usuario tras el hito 8. Los vóxeles parecían cubos grandes
  y el paisaje no se veía natural.

## Contexto

Los vóxeles ya miden 6,25 cm. Lo que delataba la cuadrícula era el paisaje:

- **Colinas demasiado suaves:** suben unos 6 m a lo largo de 87 m. Eso deja un escalón de
  un vóxel cada ~15 vóxeles, y se ven curvas de nivel por toda la pantalla.
- **Un suelo verde liso**, sin vegetación que tape los escalones.
- **Ni árboles ni rocas** en el mundo generado. Solo los había en las escenas de prueba.

## Decisiones

1. **Sigue siendo un mapa de alturas** (una altura por columna).
   - Así se conservan la caché por columna, la clasificación rápida de chunks y bricks
     uniformes y el ancla de la integridad (`GameWorld::anchor`).
   - Los salientes con ruido 3D quedan para más adelante.
2. **Relieve nuevo:**
   - **Rugosidad fina** (`roughness_*`): ±2,5 vóxeles con longitud de onda de 18. Rompe
     las curvas de nivel largas en manchas irregulares.
   - **Acantilados escalonados** (`cliff_*`): en algunas regiones, mesetas separadas por
     paredes casi verticales de ~1,5 m.
3. **Roca con estratos.** Una columna más alta o más baja que un vecino en `rock_slope`
   vóxeles o más (4) muestra roca desnuda en lugar de hierba. La roca va en bandas de
   9 vóxeles (arenisca, piedra, basalto) que ondulan con un ruido suave.
4. **Vegetación por columna.**
   - En los prados (columnas de hierba con pendiente ≤ 2), una parte de las columnas
     (`grass_density`, modulada por un ruido de prados) lleva una mata de `tall_grass`
     de 1–5 vóxeles.
   - Dentro de las manchas de flores, una parte de las matas (`flower_chance`) acaba en
     una flor roja, amarilla o violeta. El color sigue un ruido suave, así que salen
     manchas del mismo color.
   - Solo depende de la columna, así que encaja en la caché y en la clasificación por
     bricks (se guarda la altura máxima de superficie + vegetación por brick).
5. **Árboles y rocas como piezas («features»).**
   - Hay un candidato por celda de una cuadrícula: `tree_spacing` (112 vóxeles, 7 m)
     para árboles y 96 para rocas. Su posición, su tamaño y su forma salen de un hash de
     la semilla y la celda, así que un chunk sabe qué piezas le llegan sin mirar a sus
     vecinos.
   - **Árbol:**
     - 3,5–6,5 m de alto;
     - tronco de dos tramos, algo inclinado;
     - 3–5 ramas (cápsulas);
     - una copa en cada punta y otra arriba, cada una con una copa menor al lado.
   - **Copas:** son bolas con el borde deformado por ruido de valor 3D y un 18 % de
     huecos. El follaje es verde, de otoño o dorado según regiones de un ruido suave.
   - **Bosques:** un ruido de baja frecuencia decide dónde hay bosque. Solo hay árboles
     en hierba llana, por encima de la playa.
   - **Rocas:** bolas de piedra o arenisca medio enterradas, de 0,2–1 m.
   - **Qué pisan:** las piezas solo sustituyen aire y vegetación. La madera y la roca
     ganan al follaje.
6. **Materiales nuevos**, al final de `basic.ron` para no renumerar: `tall_grass`, tres
   flores, `leaves_autumn`, `leaves_gold` y `sandstone`. La vegetación arde como la
   hierba, y el follaje como las hojas (también sus reacciones con el fuego y el ácido).
7. **Campo nuevo `passable`.** Indica los sólidos que el jugador atraviesa (hierba alta y
   flores). La simulación los sigue tratando como sólidos estáticos.
8. **Versión del generador** (`terrain.generator`).
   - Las partidas guardadas antes no la tienen y se leen como generador 1: el mapa de
     alturas anterior, sin nada de lo nuevo. Así sus chunks guardados siguen encajando
     con el terreno de alrededor.
   - Las partidas nuevas guardan `generator: 2`.
9. **Brick pool de 384 MiB** (antes 256). La vegetación y los árboles suben los bricks
   densos de ~120 a ~170–200 MiB con la distancia de render 16.

## Mediciones (enchufado, RTX 5060 Laptop)

| | Antes | Ahora |
|---|---|---|
| Chunk de superficie (`terrain_surface_chunk_cached_heights`) | 100 µs | 420 µs |
| Alturas de una columna 64×64 | 0,68 ms | 1,05 ms |
| `--bench-fly` 1920×1080 | 4,05–4,19 ms\* | media 4,5–4,6 ms, p99 6,3–6,5, 0 tirones |
| Memoria (`--bench-fly`) | 132 MiB CPU, 122 MiB GPU | 177–185 MiB CPU, 164–172 MiB GPU |

**Escenas de prueba en el juego.** Todas despejan antes la vegetación de su zona
(±56 vóxeles), para que la escena se apoye en suelo desnudo:

| Escena | Generador 1 (terreno anterior) | Generador 2 |
|---|---|---|
| `--bench-fire` | media 3,6 ms, p99 6,9, máx. 9,1 (3 ticks > 8 ms) | media 4,6 ms, p99 11,8–12,0, máx. 17,7–18,3 (~110 > 8 ms) |
| `--bench-drop` | máx. 10,6 ms (31 > 8 ms) | media 5,3–5,8 ms, máx. 8,5–13,4 |

Con el generador 1 los números son los de antes: el motor no se ha vuelto más lento.
Con el generador 2 la escena da más trabajo:

- **Incendio:** el fuego alcanza los árboles generados de alrededor (1 685 frente a 668
  reacciones/tick). Al quemarse sus troncos, las copas se sueltan como cuerpos rígidos
  grandes, y la física llega a 6,6 ms en los ticks más lentos.
- **Caída de arena y agua:** sobre el suelo rugoso el agua se reparte por más chunks
  (68 activos frente a 52).

\* Antes de las normales suavizadas (ADR 010). Además, `--bench-fly` vuela ahora 10 m por
encima del punto más alto de su ruta, porque a altura fija atravesaba las montañas
nuevas.

## Tests

- **Rápido contra referencia:** la generación rápida coincide con la evaluación vóxel a
  vóxel con los dos generadores, también en los chunks que cruza un árbol.
- **Hay de todo:** en el generador 2 aparecen matas, flores, roca, árboles (tronco sobre
  el suelo, follaje) y rocas sueltas, y nunca vegetación bajo el agua.
- **Generador 1:** sin piezas, sin vegetación y con las mismas alturas que antes.
- **Partidas antiguas:** un terreno sin `generator` se lee como 1.

## Limitaciones conocidas

- **Sin salientes ni cuevas:** sigue siendo un mapa de alturas.
- **Árboles pesados:** un árbol grande tiene decenas de miles de vóxeles de follaje.
  Talarlo no siempre lo hace caer (si pasa de `integrity_max_voxels` cuenta como
  anclado). En un bosque en llamas, las copas que se sueltan como cuerpos rígidos
  disparan el coste de la física: los ticks pasan de 8 ms (ver la tabla).
- **Los cuerpos rígidos chocan con la hierba alta:** solo el jugador la atraviesa.
- **Las partidas anteriores mantienen el terreno anterior.** Para ver el nuevo hace falta
  un mundo nuevo (`--world=NOMBRE`).
