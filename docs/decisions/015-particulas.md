# 015 — Partículas: salpicaduras, espuma y chispas

- **Estado:** aceptada
- **Fecha:** 2026-11-26
- **Contexto:** segunda mitad del «agua v2» (ADR 014). Una cascada eran solo vóxeles
  de agua cayendo, sin salpicaduras.

## Decisiones

1. **Partículas solo visuales.**
   - No están en la cuadrícula ni la modifican. No se guardan ni las ve la simulación.
   - Mueren al tocar cualquier cosa que no sea aire o al acabar su vida (0,25–1 s).
2. **La simulación emite eventos** (`Simulation::take_splashes`) y sigue sin depender
   del render:
   - **`Impact`:** un líquido que cae por el aire y aterriza. Se emite en la mitad de
     los casos.
   - **`Falling`:** un líquido en caída libre. Se emite 1 de cada 16 veces.
   - El muestreo usa el hash determinista de siempre. Se guardan hasta 4 096 eventos
     entre dos lecturas.
3. **Sistema de partículas en CPU** (`mv_game::particles`):
   - **Movimiento:** gravedad de 9,81 m/s² y rozamiento con el aire.
   - **Al aterrizar:** 3 partículas que salen hacia arriba y hacia fuera.
   - **En caída libre:** 1 partícula de bruma.
   - **Color:** el de la espuma es el del líquido aclarado hacia el blanco. Los
     líquidos emisivos (lava) sueltan chispas que brillan (HDR).
   - **Tope:** `render.max_particles` (8 192; 0 las desactiva).
4. **Render** (`particles.wgsl`):
   - **Billboards:** cuadrados orientados a la cámara, uno por instancia, leídos de un
     storage buffer.
   - **Profundidad:** el raymarch guarda en el canal alfa de su salida la distancia a
     la primera superficie, y la partícula se descarta por píxel si está detrás.
   - **Mezcla:** se dibujan en un objetivo propio con alfa premultiplicado. `blit.wgsl`
     las mezcla sobre la imagen antes del tonemap. La niebla también las atenúa.
   - **Cámara:** se proyectan con la misma cámara del raymarch (el mismo uniform), así
     que coinciden píxel a píxel.

## Coste

- **Simulación:** despreciable. Un sorteo por cada vóxel líquido que cae;
  `sim_drop` sigue igual (se asienta a los 2 045 ticks, media 4,5 ms).
- **GPU:** como mucho 8 192 billboards en una pasada de pantalla completa más una
  textura `rgba16float` (unos 8 MiB a 1080p).

## Tests

- **Simulación:** el agua que cae emite `Impact` y `Falling`, la arena no, y una vez en
  reposo no hay más eventos.
- **Partículas:** se generan, caen, se desvanecen y respetan el tope; la espuma es
  más clara que su líquido.
- **Shaders:** `particles.wgsl` se valida con naga en `cargo test`.

## Limitaciones conocidas

- **Sin colisión con rebote:** las partículas desaparecen al tocar algo.
- **Sin espuma en la superficie:** no se pinta espuma sobre el agua; solo las
  partículas sobre el punto de impacto.
- **Sin iluminación:** las partículas no tienen sombras ni luz del sol propia (color
  plano, con niebla).
- **Sin comprobación visual:** no se pudo revisar en pantalla al cerrar este cambio
  (el equipo estaba bloqueado). El juego arranca y renderiza sin errores de wgpu.
