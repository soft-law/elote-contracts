# FIX fijado a la par: ¿precio por promesa del emisor? (decisión 42)

2026-11-08. Pregunta: en una serie en pesos sobre MEXe, el FIX queda fijado a la par
(`min_fix_rate == max_fix_rate == 10_000`) y, con la decisión 42, la verificación de antigüedad de
5 días (`MAX_FIX_AGE_DAYS`) se omite. ¿Es buena práctica fijar el precio de un activo a la par por
promesa del emisor, sin oráculo y sin frescura? ¿Qué riesgo residual carga elote?

**Veredicto: sólido con condiciones** (§5).

## 1. Precedentes

| Protocolo / activo | Cómo valúa el activo a la par | Qué pasó en un depeg | Confianza |
|---|---|---|---|
| MakerDAO PSM (USDC) | 1 USDC = 1 DAI en el PSM, sin oráculo; las bóvedas USDC-A usaban un precio fijo de $1 | SVB, mar-2023: la exposición a USDC se duplicó (~US$4 mil M) en un fin de semana; DAI siguió a USDC hasta ~0.88. Gobernanza reaccionó limitando el PSM (comisión de 1% USDC→DAI, menor tope diario, techos de LPs a 0) | PSM y reacción: verificados. Pip fijo de USDC-A: diseño conocido, no verificado en esta sesión |
| Compound v2 (USDC, USDT) | Precio fijo de $1 (`FIXED_USD` en `UniswapAnchoredView`) | 11-mar-2023: el Pause Guardian deshabilitó el *supply* de USDC (borrow/repay/withdraw siguieron). Pausa como respuesta al depeg, igual al patrón de elote | Pausa: verificada. `FIXED_USD`: diseño conocido, no verificado aquí |
| Aave v3 (USDC) | Oráculo de mercado Chainlink USDC/USD; CAPO acota el alza en estables | Liquidaciones posibles con USDC < $1; gobernanza congeló mercados | Conocido |
| Aave (USDe de Ethena) | USDe valuado con el feed Chainlink **USDT/USD** (par "de facto", no su mercado propio) | 10-oct-2025: USDe cayó a ~0.65 solo en el libro de Binance; en Aave no hubo cascada. Valuar por redención y no por un mercado delgado protegió de un depeg falso | Verificado (gobernanza Aave + prensa) |
| Ondo USDY / OUSG | Precio publicado por el emisor en un oráculo propio con rangos on-chain, no de mercado | Sin depeg relevante | Análogo más cercano a "promesa del emisor + bandas" |
| Maple / Centrifuge | El pool se contabiliza en su propio activo (USDC/DAI); no hay oráculo para la moneda del pool | El riesgo del estable lo carga el depositante, fuera de la contabilidad | Estructural, igual a elote |
| UST (Venus, BSC) | Oráculo de mercado con piso `minAnswer` que se comportó como precio fijo | Préstamos contra LUNA a precio irreal; deuda incobrable (cifras de prensa, no verificadas aquí) | Conocido, cifra no verificada |

Lecciones: (a) la par fija por promesa es práctica estándar en protocolos serios (Maker, Compound
v2, Aave-USDe); (b) **transmite el riesgo del emisor 1:1 al tenedor**; ninguno lo elimina; (c) la
mitigación real en cada caso fue **gobernanza rápida** (pausa, topes, comisiones), no el oráculo;
(d) el oráculo de mercado protege de un depeg verdadero pero expone a uno falso en mercados
delgados (USDe/Binance).

## 2. MEXe: lo que se sabe

- **Qué es:** "principal token" de Etherfuse que envuelve CETES; 1 MEXe = 1 MXN; el cupón se queda
  en la bóveda (docs.etherfuse.com/price-feeds; etherfuse.com/markets/mexico). **No es MXNe** (MXNe
  lo emite Brale). La referencia RedStone del repo es **MXNe**: no sirve para vigilar MEXe.
- **Emisión/redención:** onramp SPEI→MEXe y offramp MEXe→MXN vía API de Etherfuse (comisión plana
  del mercado México; sin FX). No se publican plazos, mínimos ni compuertas de redención para MEXe.
- **Respaldo y custodia:** "CETES held in a vault". BBVA aparece como custodio de reservas **de CETES**;
  las atestaciones mensuales publicadas son **de CETES**, no de MEXe.
- **Regulación:** Etherfuse invoca "CNBV guidance to issue onshore"; fuentes secundarias citan una
  resolución de no acción P090/2024 (abr-2024) sobre los stablebonds. No encontré el texto ni si
  alcanza a MEXe.
- **Mint en mainnet** (`MEXeQE5U…`, leído hoy): SPL clásico, 6 decimales, autoridad de mint
  `AiMrxQH8…` y **autoridad de congelamiento `ptaiLjcQ…`**. **Suministro: 1,902.27 MEXe** (~US$100).
- **Precio:** `/lookup/tokens/cost/mexe` devuelve `1.000000` por construcción: es el emisor
  repitiendo su promesa, no una medición. No hay mercado secundario observable ni historia de peg.
  **Un oráculo MEXe/MXN solo haría eco de la promesa**; lo que sí se puede observar es si la
  redención funciona.

## 3. Exposición real en el diseño de elote

1. **La contabilidad no ve el depeg.** NAV, depósitos, redenciones y cap están en unidades de MEXe.
   Con MEXe < 1 MXN, el pool sigue "solvente" en MEXe; la pérdida aparece **solo en la rampa de
   salida** (offramp de Etherfuse o un mercado que hoy no existe). El depositante carga el riesgo
   MEXe 1:1 sobre **toda** su posición: efectivo y cartera.
2. **El costo verdadero del fijado es la cartera.** Los pagarés son créditos en MXN. Con FIX fijo,
   cada peso adeudado se liquida con 1 MEXe; si MEXe vale 0.90 MXN, el pool renuncia a cobrar
   1.11 MEXe por peso. Con bandas ajustables, el consejo puede re-preciar (p. ej. FIX = 9_000) y
   `repay` usa el FIX vigente "whatever its age": la cartera queda protegida y la diferencia sale
   como `fx_realized`, la misma mecánica que en la serie USDC. Por eso las bandas **no** deben
   volverse inmutables.
3. **Acreditado.** Si la redención se cierra pero la emisión sigue, el acreditado compra MEXe a la
   par y paga normal: el pool acumula MEXe no redimible y el depositante queda atrapado. Si se
   cierra la emisión, el acreditado no consigue MEXe para pagar on-chain; si hubiera mercado con
   descuento, paga barato a costa del pool (punto 2).
4. **Congelamiento.** Si el emisor congela la cuenta-bóveda, `repay` falla (transferencia a cuenta
   congelada) y los mecanismos de calendario (`max_missed`, `write_off`) podrían dispararse contra
   acreditados que sí quieren pagar. Es la misma clase de riesgo que USDC, pero ya está en
   `asset_controls`.
5. **Pausa.** `paused` bloquea deposit, redeem, request, approve y draw. `repay` nunca se bloquea.
   `set_fix_bounds` y el `post_fix` del consejo **no** dependen de la pausa. La secuencia de crisis
   es posible hoy: **pausar → abrir bandas → publicar FIX nuevo → reanudar**. Al abrir las bandas,
   la frescura vuelve a aplicar (el pin desaparece) y el FIX viejo bloquea approve/draw hasta que
   se publique uno nuevo: falla cerrado.

## 4. ¿Tiene sentido la "frescura" de una constante?

Sobre el número, no: un valor fijado por las bandas no puede quedar viejo. Lo que la verificación de
5 días mide de verdad es **que la autoridad de precios esté viva**: alguien confirmó hace ≤5 días
que el activo vale lo que dice el FIX. La decisión 42 cambia el modo de falla de **cerrado**
(consejo callado → se deja de prestar) a **abierto** (consejo callado → se sigue prestando a la
par). Maker y Compound v2 operan igual: precio fijo sin latido, más un guardián que pausa. No
encontré un protocolo de préstamo que mantenga un latido sobre un precio fijo; con oráculo de
mercado el latido sí es estándar. Punto intermedio opcional: pin con antigüedad máxima larga (p. ej.
35 días, alineada a una atestación mensual). Conserva el latido a costo de un voto al mes.

## 5. Registro de riesgos MEXe

| # | Riesgo | Prob. | Impacto | Mitigación |
|---|---|---|---|---|
| M1 | Producto sin historia: 1,902 MEXe en circulación; el pool sería el tenedor dominante | Alta (hoy) | Medio | Tope de la serie vs. capacidad de redención confirmada por Etherfuse; crecer por tramos |
| M2 | Redención suspendida o lenta (Etherfuse operativo, bancario, SPEI) | Media-baja | Alto | Monitor de liveness del offramp; pausa; runbook con SLA |
| M3 | Insolvencia o fraude del emisor; reservas MEXe sin atestación propia | Baja | Muy alto | Exigir atestación específica de MEXe; divulgación; tope de serie |
| M4 | Regulatorio: MEXe tratado como fondo de pago electrónico o activo virtual no permitido; orden de la CNBV o Banxico | Media | Alto | Preguntas a counsel (§7); pausa |
| M5 | Congelamiento de la bóveda o de las cuentas de acreditados por la autoridad `ptaiLjcQ…` | Baja | Alto (repay falla, defaults falsos) | Vigilar `FreezeAccount` y `SetAuthority`; runbook para suspender el cómputo de atrasos |
| M6 | Depeg de mercado (si surge un mercado) con FIX fijo: la cartera se liquida a la par | Baja | Medio | Bandas ajustables + secuencia de crisis (§3.5) |
| M7 | Consejo inactivo con préstamos abiertos (falla abierta, decisión 42) | Baja | Medio | Monitor externo con escalamiento; latido largo opcional |
| M8 | Confusión MEXe/MXNe en monitoreo o divulgación | Media | Bajo | Vigilar el mint `MEXeQE5U…`, no el feed MXNe |

## 6. Veredicto sobre la decisión 42: sólido con condiciones

La decisión es sólida porque no existe un precio MEXe/MXN independiente que un oráculo pudiera
leer. Exigir frescura solo obliga a re-publicar 1.0000 cada 5 días por voto del consejo: un
ritual sin información. Los protocolos serios fijan la par igual. Condiciones:

1. **Bandas siempre ajustables por el consejo**; nunca inmutables ni fijadas en `init_pool` sin
   posibilidad de cambio. La secuencia pausar → abrir → re-preciar → reanudar debe probarse en
   código (test) y escribirse en el runbook.
2. **Disparo exacto:** la exención aplica solo con `min == max != 0`. Con `0, 0` (sin bandas) no
   debe aplicar. El test actual cubre solo el caso positivo; falta el negativo.
3. **Monitor off-chain de liveness, no de "precio":** cotización MEXe→MXN del offramp, latencia de
   órdenes, suministro vs. reservas, eventos `SetAuthority` y `FreezeAccount` en el mint y la
   bóveda. Alerta a los firmantes del consejo con un SLA de pausa (p. ej. ≤24 h).
4. **Tope de la serie** proporcional a la capacidad de redención confirmada por Etherfuse
   (`max_outstanding_principal` y tope de depósitos operativo) mientras M1 siga vigente.
5. **Divulgación al inversionista:** "El valor de su participación está expresado en MEXe, no en
   pesos. El pool trata 1 MEXe = 1 MXN por la promesa de redención de Etherfuse y no verifica ni
   garantiza esa redención. Si MEXe pierde su paridad o Etherfuse suspende redenciones, usted
   absorbe esa pérdida. El consejo puede pausar el pool y re-preciar."
6. Opcional: latido largo (§4) si el consejo prefiere conservar la falla cerrada.

## 7. Preguntas para counsel

1. Un token 1:1 redimible en pesos emitido por una empresa privada se parece a los **fondos de pago
   electrónico** (Ley Fintech), que solo emiten IFPE autorizadas. ¿Qué es MEXe jurídicamente? ¿La
   resolución P090/2024 lo cubre o solo cubre los stablebonds?
2. ¿Afecta la Circular 4/2019 de Banxico a que un vehículo mexicano reciba MEXe de inversionistas o
   lo entregue a acreditados?
3. El pagaré está en MXN y se entrega MEXe a la par. ¿Entregar MEXe constituye pago en pesos (art.
   8 Ley Monetaria, dación en pago)? ¿Puede el pool re-preciar unilateralmente el FIX al que acepta
   MEXe después de la emisión, o eso exige una cláusula expresa en el contrato de crédito?
4. Si la bóveda se congela y el acreditado no puede pagar on-chain, ¿hay mora? Se necesita una
   cláusula de pago alternativo (SPEI a cuenta del vehículo).
5. ¿Qué responsabilidad tiene el vehículo frente a inversionistas por la elección de MEXe? ¿Basta la
   divulgación de §6.5?

Fuentes: docs.etherfuse.com/price-feeds y /v2/fees; etherfuse.com/markets/mexico; RPC mainnet
(getAccountInfo `MEXeQE5U…`, 2026-11-08); api.etherfuse.com/lookup/tokens/cost/mexe;
governance.aave.com (USDe price feed, ARFC); Gallifrey Labs, Compound marzo 2023; Forklog/NewsBTC
sobre MakerDAO y SVB; protos.com y cointelegraph sobre USDe en Binance.
