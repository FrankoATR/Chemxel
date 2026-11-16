# 004 — Almacenamiento de chunks por bricks

- **Estado:** aceptada
- **Fecha:** 2026-11-16
- **Hito:** 2

## Contexto

PROMPT.md §5.2 propone un array denso de 64³ materiales por chunk, más una variante
uniforme para chunks enteros de un solo material. Un chunk denso pesa 512 KiB (`u16` por
vóxel).

El hito 2 exige un radio de render de ≥ 16 chunks. Eso son unas 800–1 100 columnas de
chunks, y en cada columna la superficie del terreno y la del agua cruzan 1–3 chunks no
uniformes. Con arrays densos serían ~1 600–3 000 chunks × 512 KiB ≈ **0,8–1,5 GB de RAM**
solo de materiales, casi todo piedra o aire interior.

## Decisión

Cada chunk tiene dos niveles de compresión:

```rust
enum ChunkData { Uniform(MaterialId), Bricks(Box<[Brick; 512]>) }
enum Brick     { Uniform(MaterialId), Dense(Box<[MaterialId; 512]>) }
```

- El brick de almacenamiento coincide con el brick de render (8³). Así la subida a GPU
  es directa y un brick uniforme no se empaqueta.
- Un chunk con bricks ocupa 8 KiB de tabla más 1 KiB por brick denso. Por eso la memoria
  crece con la **superficie**, no con el volumen.
- Índices: `LocalPos::brick_index` (brick dentro del chunk) y
  `LocalPos::brick_local_index` (vóxel dentro del brick). Ambos son x-rápido, igual que
  el resto del motor.
- `Chunk::set` densifica solo el brick tocado. `Chunk::compact` reconvierte los bricks
  (y el chunk) que vuelven a ser uniformes.
- Flag `modified`: indica si el chunk cambió desde que se generó. Un chunk modificado no
  se descarta al alejarse; se guarda en memoria y en el hito 8 se persistirá.

## Alternativas descartadas

- **Array denso por chunk (spec literal).** La opción más simple, pero no cumple un
  presupuesto de memoria razonable a 16 chunks de radio.
- **Paleta `u8` por chunk.** Reduce a la mitad, pero sigue siendo proporcional al
  volumen.
- **Octree / SVO.** Más compacto, pero el acceso y la escritura son mucho más caros, y
  la simulación del hito 3 escribe vóxeles constantemente.

## Consecuencias

- `get`/`set` hacen un salto extra (tabla → brick). La simulación del hito 3 debería
  trabajar brick a brick para aprovechar la localidad.
- Medición (hito 2, radio 16/8): 14 586 chunks cargados, de los cuales 13 324 son
  uniformes, y 125 312 bricks densos. En total ocupan **132 MiB** de CPU. Los 1 262 chunks
  no uniformes ocuparían ~630 MiB como arrays densos. Calcular la ocupación de bricks de un
  chunk pasó de 31 µs a 1 µs.
