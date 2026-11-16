# mv_materials

Registro de materiales de Chemxel. Carga todos los `assets/materials/*.ron` (en orden
alfabético de archivo, para que los ids sean deterministas) y los guarda en una tabla
contigua indexada por `MaterialId` (`u16`).

## Reglas

- Debe existir un material llamado `"air"`: siempre recibe el id `0`, así que un array de
  vóxeles inicializado a cero significa "vacío".
- Los nombres son únicos entre todos los archivos.
- Las referencias a otros materiales (`melts_into`, `boils_into`, `freezes_into`,
  `burns_into`) deben existir; si no, la carga falla indicando material, archivo y campo.
- Cada transición se escribe en pareja: `melt_at` + `melts_into`, `boil_at` + `boils_into`,
  `freeze_at` + `freezes_into`, `ignite_at` + `burns_into`.
- Los campos desconocidos son un error (detecta erratas).
- Extensión RON `implicit_some` activa: `melt_at: 1200` equivale a `melt_at: Some(1200)`.

## Esquema de `Material`

| Campo | Tipo | Por defecto | Significado |
|---|---|---|---|
| `name` | texto | — (obligatorio) | Identificador único |
| `state` | `Solid` \| `Powder` \| `Liquid` \| `Gas` \| `Energy` | — (obligatorio) | Comportamiento físico |
| `density` | número | — (obligatorio) | kg/m³; decide quién se hunde y quién flota |
| `color` | `(r, g, b, a)` 0–255 | — (obligatorio) | Color sRGB; `a` < 255 = translúcido |
| `conductivity` | número | `0` | Conductividad térmica, W/(m·K) |
| `heat_capacity` | número | `1.0` | Capacidad calorífica, kJ/(kg·K) |
| `emissive` | número | `0` | Intensidad de luz emitida |
| `dispersion` | entero 0–255 | `0` | Celdas que un líquido/gas se expande en horizontal por tick |
| `lifetime` | `(min, max)` ticks | ninguno | Vida aleatoria; al expirar el vóxel desaparece |
| `base_temperature` | °C | ninguno | Temperatura con la que se crea y a la que tiende |
| `melt_at` / `melts_into` | °C / nombre | ninguno | Fusión (al superar la temperatura) |
| `boil_at` / `boils_into` | °C / nombre | ninguno | Ebullición (al superar la temperatura) |
| `freeze_at` / `freezes_into` | °C / nombre | ninguno | Congelación / condensación (al bajar de la temperatura) |
| `flammable` | bool | `false` | Puede arder |
| `ignite_at` / `burns_into` | °C / nombre | ninguno | Ignición y producto de la combustión |
| `burn_rate` | 0–1 | `0` | Probabilidad por tick de consumirse mientras arde |

## Ejemplo

```ron
[
  Material(name: "air",   state: Gas,    density: 1.2,  color: (0, 0, 0, 0)),
  Material(name: "water", state: Liquid, density: 1000, color: (40, 90, 200, 180),
           heat_capacity: 4.18, dispersion: 5,
           boil_at: 100, boils_into: "steam", freeze_at: 0, freezes_into: "ice"),
]
```

Las reacciones químicas (`assets/reactions/*.ron`) se añaden en el hito 5.
