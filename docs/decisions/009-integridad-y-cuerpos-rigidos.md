# 009 — Integridad estructural y cuerpos rígidos de vóxeles

- **Estado:** aceptada
- **Fecha:** 2026-11-18
- **Hito:** 6

## Contexto

PROMPT.md §6.6 y el hito 6 piden:
- un flood fill acotado que encuentre los componentes sólidos sin conexión con el suelo;
- convertir esos componentes en cuerpos rígidos de `rapier3d`, con colisionadores
  derivados de los vóxeles, dibujarlos con su transformación y revoxelizarlos al
  dormirse.

**Aceptación:** excavar la base de una torre la hace caer como un bloque y, al chocar,
queda integrada al mundo.

## Decisiones

1. **La integridad vive en `mv_sim`, headless** (`integrity.rs`).
   - Una comprobación se dispara cuando desaparece un sólido (material `Solid`; polvos y
     líquidos no son estructurales):
     - ediciones del jugador (`Simulation::edited` con `placed = None`);
     - reacciones;
     - combustión;
     - fusión;
     - lo que el motor de física devuelve a la rejilla.
   - El relleno es 6-conexo sobre sólidos estáticos.
2. **Qué es «suelo».** Un componente está anclado si el relleno:
   - llega a un vóxel ancla: predicado `Anchor`; en el juego, más de
     `world.anchor_depth` (32) vóxeles por debajo de la superficie generada, con las
     alturas por columna que ya cachea el generador;
   - toca un chunk sin cargar;
   - o supera `integrity_max_voxels` (65 536) o `integrity_max_extent` (192 por eje).

   Lo enorme es terreno, y así el relleno nunca recorre el mundo entero.
3. **Orden del relleno: primero en altura.** La frontera es una cola de prioridad por
   `y`, en orden de inserción dentro de cada nivel.
   - Una pieza apoyada encuentra el ancla en pocas decenas de pasos.
   - Una copa encuentra su tronco más cercano en lugar de vagar por copas que se tocan.
   - Con una DFS «abajo primero», el bosque de prueba recorría hasta 25 000 vóxeles por
     comprobación.
4. **Presupuesto y lotes.**
   - `integrity_budget_voxels` (262 144) por tick. Una región empezada se termina, para
     que todas avancen; las siguientes esperan.
   - Las desapariciones «naturales» (fuego, ácido, fusión) se guardan como puntos y se
     comprueban juntas cada `integrity_natural_period` (8) ticks, sembrando solo desde
     sus vecinos sólidos. Las ediciones van como cajas (un trazo de pincel es pequeño).
   - Lo que ya se sabe anclado se comparte durante toda la pasada.
5. **Migas.** Las piezas sueltas de menos de `integrity_min_voxels` (64) se quedan donde
   están. El fuego y el ácido van dejando miles de fragmentos de un vóxel: sin este
   límite, el bosque en llamas generaba 4 054 cuerpos (64 simultáneos) y ticks de 22 ms.
6. **Extracción.**
   - La pieza sale de la rejilla como `VoxelBody`: rejilla densa con el material de cada
     vóxel y aire donde no hay.
   - El hueco despierta la simulación: lo que se apoyaba en la pieza cae.
   - Su energía térmica se va con ella, sin liberar calor latente; queda contabilizada en
     `energy_imbalance`.
7. **Física (`mv_physics`, rapier3d 0.36, en la lista de PROMPT.md §3).**
   - **Cuerpo:** cada pieza es un cuerpo dinámico con un colisionador `Voxels` de parry
     (su propia rejilla, vóxel de 1/16 m). La masa sale de la densidad media de sus
     materiales.
   - **Mundo:** alrededor de cada cuerpo, un colisionador estático `Voxels` con los
     sólidos y polvos de su caja más `world_margin` (12 vóxeles). Se reconstruye cuando el
     cuerpo sale de esa región o cada 15 ticks.
   - **Grupos de colisión:** cada colisionador del mundo solo toca a su cuerpo, así que las
     regiones solapadas no duplican contactos. Los cuerpos chocan entre sí.
   - **Subpasos:** un colisionador de vóxeles solo genera contactos en su superficie, así
     que un cuerpo que la atraviesa en un paso cae a través del suelo (pasaba al caer
     19 vóxeles). Cada tick se divide para que ningún cuerpo avance más de medio vóxel por
     subpaso, con un máximo de `max_substeps` (16), y la velocidad se limita a lo que eso
     permite (~15 m/s). Se añade además un *soft CCD* de dos vóxeles.
   - **Origen de física:** las posiciones son relativas a un origen que se recentra cuando
     no hay cuerpos, así `f32` es preciso en cualquier punto del mundo infinito.
8. **Revoxelizar.**
   - Un cuerpo más lento que `settle_speed` durante `settle_ticks` (o con más de
     `max_body_seconds`) vuelve a la rejilla por mapeo inverso: cada celda del mundo
     cuyo centro cae dentro de un vóxel lleno del cuerpo recibe ese material. No quedan
     agujeros tras una rotación.
   - Solo se escribe sobre aire, líquidos y gases; si no hay sitio, el vóxel se pierde y
     se cuenta.
   - Se vuelve a comprobar la integridad ahí: la pieza podría haber quedado colgando.
   - Si no caben más cuerpos (`max_bodies`), la pieza se devuelve a su sitio sin repetir
     la comprobación, para que no se extraiga en bucle.
9. **Render.**
   - Cada cuerpo sube con su rotación inversa, posición relativa a la ventana, tamaño y
     desplazamiento en un pool de `u16`, empaquetado como el brick pool.
   - Los vóxeles solo se reempaquetan cuando cambia el conjunto de cuerpos; las
     transformaciones se reescriben cada frame.
   - El shader traza los cuerpos tras el mundo (prueba de slab contra su caja y DDA de
     vóxeles en espacio local) y gana el impacto más cercano. El sombreado usa la normal
     girada.

## Mediciones (RTX 5060 Laptop / 24 hilos)

| Escenario | Resultado |
|---|---|
| Test de aceptación (torre 8×40×8, base excavada) | Cae recta (inclinación < 0,1 rad) y queda quieta en 23 ticks; 2 304 de 2 304 vóxeles revoxelizados |
| Viga en voladizo | Vuelca (0,15 rad) y queda integrada |
| `--bench-tower` (torre de 16×96×16 en el terreno, base excavada y un foso bajo media torre), enchufado | Media 0,21 ms/tick, p99 3,73, máx. 7,55 (0 ticks > 8 ms). La torre vuelca, se tumba y vuelve a la rejilla |

**Incendio con colapsos** (`sim_fire`: bosque 6×6 con física; las copas caen al quemarse
los troncos):
- Se sueltan 137 piezas, con un máximo de 9 cuerpos a la vez.
- Medido **con batería** (la CPU va más lenta y los tiempos son ×2–2,5 los de
  enchufado): media 4,34 ms/tick, p95 11,2, máx. 17,2.
- El hito 5, sin integridad ni física y en las mismas condiciones, daba media 3,8,
  p95 9,8 y máx. 12,2. La integridad y la física suman ~15 % de media.
- Los picos son la creación de cuerpos: construir el colisionador del mundo cuando cae
  una copa.
- **Pendiente:** repetir enchufado.

## Limitaciones conocidas

- **Migas colgando.** Las piezas de menos de 64 vóxeles no caen. Un trozo pequeño
  arrancado con el pincel se queda flotando.
- **Sin fractura.** Una pieza solo se suelta si se separa del todo; un corte parcial no
  rompe nada por esfuerzo.
- **Polvos y líquidos.** Los polvos colisionan con los cuerpos como sólidos y los
  líquidos no colisionan. Al revoxelizar se sobrescriben líquidos y gases, así que el
  agua desplazada se pierde.
- **Cuerpos congelados.** Mientras vuela, un cuerpo no se simula: un tronco en llamas
  deja de arder hasta volver a la rejilla.
- **Revoxelizado aproximado.** Una pieza girada se aproxima a la rejilla; el número de
  vóxeles cambia ±1 %.
- **Ancla geométrica.** Bajo `anchor_depth` nada cae, aunque se excave alrededor.
