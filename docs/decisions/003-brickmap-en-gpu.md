# 003 — Formato del brickmap en GPU y pipeline de render

- **Estado:** aceptada
- **Fecha:** 2026-11-16
- **Hito:** 1

## Contexto

PROMPT.md §7.1 fija el enfoque (raymarching en compute shader sobre brickmaps, DDA
jerárquico) pero deja abiertos el tipo de recurso GPU, el empaquetado y cómo se
direccionan chunks y bricks. Las decisiones deben servir ya para el hito 2 (muchos chunks,
subida incremental) sin rehacerlas.

## Decisiones

1. **Storage buffers, no texturas 3D.** Tres buffers con indirección explícita:
   - `chunk_grid`: una entrada `u32` por celda del volumen renderizado → *slot* de chunk o
     `EMPTY` (`0xFFFFFFFF`).
   - `chunk_bricks`: 512 entradas `u32` por slot de chunk → slot de brick o `EMPTY`.
   - `brick_pool`: 512 materiales `u16` por brick, **dos por `u32`** (mitad baja = índice
     par). 1 KiB por brick.
   Los buffers permiten asignar y liberar bricks individuales con un `SlotAllocator`
   (lista libre LIFO) sin fragmentar un atlas 3D, y `queue.write_buffer` sobre rangos.
2. **Los bricks vacíos no ocupan memoria.** `Chunk::copy_brick` informa si el brick tiene
   algo distinto de aire; si no, su slot se libera.
3. **Orden de índices siempre x-rápido** (`x + y*N + z*N²`) en todos los niveles, igual que
   en la CPU.
4. **Material como `u16` sin paleta** por ahora. La paleta por brick (`u8`) queda como
   optimización futura si la memoria lo exige.
5. **Tabla de materiales en GPU** (`MaterialGpu`: color lineal + emisivo), construida por
   el juego desde el `MaterialRegistry`. `mv_render` no depende de `mv_materials`
   (`mv_world` reexporta `MaterialId`).
6. **DDA de dos niveles** (bricks 8³ → vóxeles). El salto de chunks vacíos queda implícito
   en la búsqueda `chunk_grid` (entrada `EMPTY` ⇒ brick vacío); un tercer nivel explícito
   se añadirá si las mediciones del hito 2 lo justifican.
7. **Cámara en `f64`, GPU en coordenadas relativas.** La posición se guarda como `DVec3` en
   microvóxeles y se resta el origen del volumen en `f64` antes de pasarla a `f32`, así la
   precisión no se degrada lejos del origen. Los rayos se generan con la base
   forward/right/up y `tan(fov/2)`, sin matriz inversa.
8. **Salida HDR `rgba16float`** escrita por el compute shader, y un pase de blit que aplica
   tonemap ACES y escribe a la swapchain sRGB (la conversión sRGB la hace el hardware).
9. **Shaders embebidos con `include_str!`** desde `assets/shaders/`. La recarga en caliente
   de shaders se puede añadir más adelante leyendo el mismo archivo del disco.

## Consecuencias

- Hito 2 solo necesita: un volumen de N³ chunks que se desplace con la cámara, subir
  chunks nuevos, liberar los que salen y un presupuesto de bytes por frame.
- Un chunk uniforme no vacío (todo piedra) ocupa hoy 512 bricks; si eso pesa en memoria,
  se marcará como uniforme directamente en `chunk_grid`.
- Medición de referencia (hito 1, RTX 5060 Laptop, 1 chunk, 1920×1080, sin vsync):
  ~3 600 FPS (0,28 ms/frame).
