# 011 — Jugador con colisión y guardado del mundo

- **Estado:** aceptada
- **Fecha:** 2026-11-19
- **Hito:** 8

## Contexto

El hito 8 pide:
- un controlador de personaje con colisión contra vóxeles y modos de vuelo y caminar;
- guardar y cargar el mundo: chunks modificados comprimidos, con formato versionado.

**Aceptación:** salir y volver a entrar conserva todos los cambios. PROMPT.md §5.2 propone
«paleta por chunk + RLE o LZ4».

## Decisiones

1. **Formato de chunk** (`mv_world::persist`). Sin dependencias nuevas:

   ```
   "CXCH" · versión u16 · paleta (u16 nº + nombre de cada material) ·
   rachas (índice de paleta, longitud) en varint LEB128, en orden de almacenamiento
   (brick, luego vóxel) · Adler-32
   ```

   - **Por nombre:** los materiales se guardan por nombre, no por id, así que una partida
     sobrevive a que se añadan o reordenen materiales. Los nombres desconocidos se
     cargan como aire, con un aviso.
   - **Tamaño:** un brick uniforme es una sola racha. Un chunk de terreno editado ocupa
     de unos cientos de bytes a pocos KiB; la torre derribada (10 chunks) ocupó 44,7 KiB.
   - **Errores:** una versión más nueva, un archivo truncado o un checksum incorrecto
     se rechazan con un error claro, y el chunk se regenera.
2. **Almacén** (`mv_world::store`).
   - Un archivo por chunk modificado en `saves/<mundo>/chunks/x_y_z.cxc`.
   - Se escribe en un temporal y se renombra, así un corte a mitad no deja un chunk roto.
   - Al abrir se lista qué hay guardado, para que el streaming sepa sin tocar el disco si
     cargar o generar.
   - Más adelante se podrán agrupar en archivos de región, si hay demasiados archivos.
3. **Streaming con disco.**
   - El chunk guardado se decodifica en el mismo trabajo de rayon que lo habría
     generado.
   - Al descargar un chunk modificado con cambios sin guardar, se escribe a disco en vez
     de quedarse en el stash de memoria (el stash sigue para cuando no se guarda).
   - `Chunk` lleva una marca de «cambios sin guardar»: el guardado solo escribe lo que
     cambió.
4. **Cabecera `world.ron`**:
   - versión;
   - semilla y configuración de terreno con que se generó el mundo, que tienen prioridad
     sobre `config.ron`, para que los chunks guardados encajen con el terreno de
     alrededor;
   - tick;
   - posición, orientación y modo del jugador.
5. **Las temperaturas no se guardan.**
   - Un chunk que vuelve (de disco o del stash) pasa por `Simulation::chunk_restored`:
     se despierta entero (lo que caía sigue cayendo) y sus materiales con
     `base_temperature` la recuperan. La lava sigue fundida y el hielo congelado.
   - Es más simple y coherente que serializar los campos térmicos.
6. **Cuándo se guarda.**
   - Autoguardado cada `world.autosave_seconds` (60), pero nunca con cuerpos rígidos en
     vuelo, porque sus vóxeles están fuera de la rejilla: espera a que aterricen.
   - Al salir (cerrar la ventana, Esc, fin de un benchmark), los cuerpos en vuelo se
     revoxelizan donde están y se guarda.
   - Botón «Save now» en el overlay.
   - `--world=NOMBRE` elige la partida (por defecto `default`); `--no-save` desactiva el
     guardado.
   - Los benchmarks no guardan salvo que se nombre un mundo. `CHEMXEL_SAVES_DIR` cambia
     la carpeta.
7. **Jugador** (`mv_world::collide` + `mv_game::player`).
   - **Caja:** 0,6 × 1,8 m con los ojos a 1,62 m, movida eje a eje en pasos de ≤ ½ vóxel
     contra sólidos y polvos.
   - **Movimiento:** gravedad, salto (1,1 m) y escalones automáticos de hasta 0,5 m (el
     terreno es una escalera de terrazas de un vóxel).
   - **Agua:** se nada despacio (Espacio sube).
   - **Mundo sin cargar:** los chunks sin cargar cuentan como sólidos, para no caer al
     vacío mientras llegan. El jugador espera quieto hasta que se carga el suelo.
   - **Material colocado:** si el jugador queda dentro, sube hasta salir.
   - **Modos:** F alterna caminar y volar; el vuelo es la cámara libre de siempre, sin
     colisión. Todo se ajusta en `controls`.

## Mediciones y aceptación

- **Tests:**
  - formato (ida y vuelta de chunks uniformes, editados y con ruido; materiales por
    nombre; archivos dañados; Adler-32);
  - almacén;
  - **reinicio a nivel de mundo**: se cambian vóxeles, uno se aleja (los chunks se
    escriben), y un streamer nuevo sobre el mismo directorio los recupera;
  - colisión (caer, paredes, escalones, agua, mundo sin cargar, desatascar);
  - jugador (aterriza, salta 0,9–1,4 m, sube terrazas);
  - cabecera (ida y vuelta, rechaza versiones nuevas y nombres peligrosos);
  - `chunk_restored`.
- **En el juego:**
  - Sesión 1: `--world=accept --bench-tower` derriba la torre y al salir guarda 10
    chunks (44,7 KiB) en 35 ms.
  - Sesión 2: `--world=accept` arranca con la torre tumbada y el foso, la cámara donde
    estaba y «10 chunks on disk».
  - Un mundo nuevo en modo caminar: el jugador cae al terreno y se queda de pie.
  - Cerrar la ventana guarda.

## Limitaciones conocidas

- **Temperaturas y sim:** las temperaturas, la región activa de la simulación y las
  piezas en vuelo no se guardan tal cual. Las piezas se aterrizan al salir.
- **Archivos sueltos:** un archivo por chunk; con miles de chunks editados convendría
  agrupar en regiones.
- **Jugador:** no choca con los cuerpos rígidos en vuelo ni los empuja.
- **Cabecera:** el tick se guarda como información; la simulación vuelve a empezar en 0
  (no afecta a nada visible).
