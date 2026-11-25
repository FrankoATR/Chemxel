# mv_materials

Registro de materiales y reacciones de Chemxel.

- `MaterialRegistry::load_dir` carga todos los `assets/materials/*.ron` (en orden
  alfabético de archivo, para que los ids sean deterministas) y los guarda en una tabla
  contigua indexada por `MaterialId` (`u16`).
- `ReactionSet::load_dir` carga después los `assets/reactions/*.ron` y construye una
  matriz `a × b` para buscar reacciones en O(1).

## Reglas

- Debe existir un material llamado `"air"`: siempre recibe el id `0`, así que un array de
  vóxeles inicializado a cero significa "vacío".
- Los nombres son únicos entre todos los archivos.
- Las referencias a otros materiales (`melts_into`, `boils_into`, `freezes_into`,
  `burns_into`) deben existir; si no, la carga falla indicando material, archivo y campo.
- Cada transición se escribe en pareja: `melt_at` + `melts_into`, `boil_at` + `boils_into`,
  `freeze_at` + `freezes_into`, `ignite_at` + `burns_into`.
- Las probabilidades (`burn_rate`, `flame_rate`, `probability`) y `heat_loss` van de 0 a 1;
  `lifetime` necesita `0 < min <= max`, y `decays_into` necesita un `lifetime`.
- **Hot reload** (builds de debug): el juego vuelve a cargar los RON cuando cambian. Un
  material existente no puede cambiar de id (no se pueden quitar ni reordenar materiales,
  solo añadir al final o cambiar propiedades); si no, la recarga se rechaza con un error
  en el log y se sigue con la versión anterior.
- Los campos desconocidos son un error (detecta erratas).
- Extensión RON `implicit_some` activa: `melt_at: 1200` equivale a `melt_at: Some(1200)`.

## Esquema de `Material`

| Campo | Tipo | Por defecto | Significado |
|---|---|---|---|
| `name` | texto | — (obligatorio) | Identificador único |
| `state` | `Solid` \| `Powder` \| `Liquid` \| `Gas` \| `Energy` | — (obligatorio) | Comportamiento físico |
| `category` | texto | según `state` (`Solids`, `Powders`, `Liquids`, `Gases`, `Energy`) | Pestaña del inventario donde aparece |
| `density` | número | — (obligatorio) | kg/m³; decide quién se hunde y quién flota |
| `color` | `(r, g, b, a)` 0–255 | — (obligatorio) | Color sRGB; `a` < 255 = translúcido |
| `conductivity` | número | `0` | Conductividad térmica, W/(m·K) |
| `heat_capacity` | número | `1.0` | Capacidad calorífica, kJ/(kg·K) |
| `latent_heat` | número | `0` | Energía latente por vóxel respecto a la fase más baja; en combustibles, la que libera al arder (ver abajo) |
| `heat_loss` | 0–1 | `0` | Calor perdido al entorno por vóxel, tick y grado sobre el ambiente (convección que la grilla no modela; sobre todo el aire) |
| `emissive` | número | `0` | Intensidad de luz emitida |
| `dispersion` | entero 0–255 | `0` | Líquidos: celdas que se expanden en horizontal por tick. Gases: celdas que suben por tick |
| `lifetime` | `(min, max)` ticks | ninguno | Vida aleatoria; al expirar el vóxel se transforma en `decays_into` |
| `decays_into` | nombre | `air` | Producto cuando expira el `lifetime` |
| `evaporates_into` / `evaporate_rate` | nombre / 0–1 | ninguno | Líquidos: en qué se convierte un vóxel suelto de la superficie en reposo (gota, bulto sobre una capa llena, borde de un hueco en una capa incompleta) y probabilidad por tick |
| `ceiling` / `ceiling_into` | altura (vóxeles) / nombre | ninguno | Gases: a esa altura del mundo o más se convierten en otro material (vapor → nube) |
| `drifts` | bool | `true` | Gases: si se mueven; `false` = quieto (una nube) |
| `spreads` | bool | `true` | Gases: si se desplazan a los lados cuando no pueden subir; `false` = el vapor atrapado bajo un techo se queda quieto |
| `base_temperature` | °C | ninguno | Temperatura con la que se crea y a la que tiende |
| `transition_rate` | 0–1 | `sim.transition_rate` (0,1) | Probabilidad por tick de sus cambios de fase (fundirse, hervir, solidificarse) una vez pasado el umbral; más bajo = más lento |
| `melt_at` / `melts_into` | °C / nombre | ninguno | Fusión (al superar la temperatura) |
| `boil_at` / `boils_into` | °C / nombre | ninguno | Ebullición (al superar la temperatura) |
| `freeze_at` / `freezes_into` | °C / nombre | ninguno | Congelación / condensación (al bajar de la temperatura) |
| `flammable` | bool | `false` | Puede arder |
| `ignite_at` / `burns_into` | °C / nombre | ninguno | Ignición y producto de la combustión |
| `burn_rate` | 0–1 | `0` | Probabilidad por tick de consumirse mientras arde |
| `flame` | nombre | ninguno | Material que emite al aire vecino mientras arde (p. ej. `fire`); su `base_temperature` limita la temperatura de la combustión |
| `flame_rate` | 0–1 | `0` | Probabilidad por tick de emitir una llama mientras arde |

Un vóxel inflamable **arde** mientras su celda de temperatura supera `ignite_at`. Cada
tick, con probabilidad `burn_rate`, se convierte en `burns_into` y libera
`latent_heat(material) − latent_heat(producto)`, hasta la temperatura de su llama. Si no
se consume, con probabilidad `flame_rate` emite una llama en un vóxel de aire vecino.

`lifetime` se implementa sin guardar la edad de cada vóxel: cada tick el vóxel expira con
probabilidad `2 / (min + max)`, es decir, con vida media `(min + max) / 2` ticks (ADR 008).

## Unidades térmicas

La simulación (ADR 007) usa como capacidad calorífica de un vóxel `density × heat_capacity`
(energía por kelvin). `latent_heat` usa esas mismas unidades: un cambio `A → B` absorbe
`latent_heat(B) − latent_heat(A)` de su celda de temperatura, o lo libera si es negativo.
Los valores son de juego, no físicos. Un vóxel conserva su volumen al cambiar de fase, así
que con los valores reales el vapor al condensarse calentaría el aire miles de grados.

## Ejemplo

```ron
[
  Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
  Material(name: "water", state: Liquid, density: 1000, color: (40, 90, 200, 180),
           heat_capacity: 4.18, dispersion: 5,
           boil_at: 100, boils_into: "steam", freeze_at: 0, freezes_into: "ice"),
]
```

## Esquema de `Reaction`

`a + b → result_a + result_b`. La simulación evalúa la reacción al recorrer un vóxel de
`a` que tiene a `b` como vecino de cara. Por eso `a` debería ser un material que se mueve
(líquido, gas, polvo o fuego): un sólido estático en un brick uniforme no se recorre. Cada
par `(a, b)` solo puede definirse una vez; `(b, a)` es otra reacción.

| Campo | Tipo | Por defecto | Significado |
|---|---|---|---|
| `a`, `b` | nombre | — (obligatorio) | Reactivos |
| `result_a`, `result_b` | nombre | el mismo reactivo | Productos |
| `probability` | 0–1 | — (obligatorio) | Probabilidad por tick y contacto |
| `heat` | número | `0` | Energía extra liberada (> 0) o absorbida (< 0), en las unidades de `latent_heat` |
| `min_temp`, `max_temp` | °C | sin límite | Rango de temperatura requerido en la celda de `a` |

Los productos absorben o liberan además la diferencia de `latent_heat`, como en un cambio
de fase: la lava que se convierte en obsidiana libera su calor latente.

```ron
[
  Reaction(a: "water", b: "lava", result_a: "steam", result_b: "obsidian", probability: 0.3),
  Reaction(a: "fire",  b: "wood", probability: 0.3, heat: 300000),
  Reaction(a: "acid",  b: "stone", result_a: "air", result_b: "air", probability: 0.02),
]
```
