# Circuitos y zero-knowledge en Solana (nov 2026): qué le sirve a elote

2026-11-09. Investigación de escritorio con verificación on-chain mínima. Pregunta: ¿alguna primitiva
ZK de Solana resuelve hoy un problema real de `elote_pool` (registro pseudónimo del pagaré, Z1;
retención vs borrado a 72 meses, P-94; KYC con Didit; inversionistas gated por SAS + Token ACL)?
**Respuesta corta: no.** Los primitivos existen y funcionan, pero ninguno ataca un problema que
elote tenga hoy, y el único que se parece (montos privados) choca de frente con el diseño del pool.

Fuentes: primarias cuando se pudo (docs de crates, repos, auditorías, post-mortem); solana.com
devolvió 429 durante la sesión, así que varias fechas vienen de prensa secundaria y se marcan así.

## 1. Primitivas nativas

**Verificación on-chain (feature gates).** `solana-cli 4.1.2` → `solana feature status -um`
(2026-11-09): la lista de gates *pendientes* de mainnet no incluye alt_bn128, Poseidon, ZK ElGamal,
BN254-G2 (SIMD-0302) ni BLS12-381 (SIMD-0388) — consistente con que estén activados y ya limpiados
del feature set. Lo único inactivo relacionado es el viejo *ZK Token Proof program* (`zk1snx…`,
`zkNLP…`, `zkiTN…`), deprecado y reemplazado por ZK ElGamal. Mainnet corre Agave 4.4 (~83% del
stake) y Firedancer 26.10 (~16%).

- **alt_bn128 (G1 add/mul + pairing):** activo desde la serie 1.18 (2024). Es lo único que
  Groth16 necesita; la crate `groth16-solana` (Light) dice "menos de 200 000 CU" por verificación,
  auditada en la auditoría de Light v3. Rango realista 170k–500k CU según el número de entradas
  públicas (Chainstack). Cabe en una tx (límite 1.4M CU) pero exige `SetComputeUnitLimit`.
  — https://docs.rs/crate/groth16-solana/latest · https://docs.chainstack.com/docs/solana-zk-proofs
- **SIMD-0302 (G2 standalone, epoch 985) y SIMD-0388 (BLS12-381, epoch 986)**, ambas en Agave 4.0
  (ago 2026). Son *extensiones*: Groth16 ya era practicable sin ellas. Ojo con la prensa que dice
  que 0302 "habilita Groth16" — es engañoso.
  — https://solanacompass.com/news/solana-activates-bn254-bls12-381-and-zk-elgamal-three-cryptographic-primitives-now-live
- **`sol_poseidon`:** syscall aportada por Light (PR solana-labs#32680), compatible con circomlib
  BN254, 1–12 entradas. Usada en mainnet por ZK Compression en cada slot. Caveat: el Poseidon de la
  stdlib de Noir **no** es byte-compatible; hay que usar `noir-lang/poseidon`. SIMD-0359 (entrada
  exacta de 32 bytes) seguía pendiente en mainnet en abril 2026.
  — https://github.com/solana-labs/solana/pull/32680 · https://solana.com/docs/core/programs/syscall-reference
- **ZK ElGamal Proof program:** deshabilitado el 2025-06-19 (epoch 805) por un fallo de
  Fiat-Shamir (un componente no entraba al hash del transcript); "sin valor en riesgo". Auditoría
  competitiva Code4rena (ago–sep 2025, $203.5k) además de Least Authority, zkSecurity y QEDIT
  sobre Confidential Transfer. **Re-habilitación: dos eventos, no una contradicción** — el gate
  del proof program se activó ~4 jun 2026 (epoch 982; 177 epochs ≈ 354 días después de la 805,
  cuadra) y las instrucciones confidenciales de Token-2022 se re-desplegaron después, reportado
  17 jun. El gate de deshabilitar sigue existiendo: ya se usó una vez y se puede volver a usar.
  — https://solana.com/news/post-mortem-june-25-2025 ·
  https://solanacompass.com/news/confidential-transfers-return-to-solana-mainnet-after-year-long-security-pause ·
  https://dev.to/sulimanmukhtar/confidential-transfers-have-a-kill-switch-what-it-costs-you-5h39 ·
  https://leastauthority.com/wp-content/uploads/2025/11/Least-Authority-Token-2022-Confidential-Transfer-Final-Audit-Report.pdf ·
  https://reports.zksecurity.xyz/reports/anza-solana-token2022/

## 2. Token-2022 Confidential Transfers

Qué oculta: **montos y saldos**, cifrados con ElGamal twisted. Qué no: emisor, receptor, hora, y
los montos de `Deposit` (público→confidencial) y `Withdraw` (confidencial→público), porque mueven el
saldo público. Flujo: `ConfigureAccount` con la ElGamal key del dueño + prueba de validez →
`Deposit` → `ApplyPendingBalance` → `Transfer` (3 pruebas: igualdad, validez, rango, normalmente
en context accounts y varias tx; tx v1 de 4 096 bytes ayuda) → `Withdraw` (2 pruebas). La mint
puede fijar una **auditor ElGamal pubkey** que descifra todos los montos sin poder gastar (QEDIT
reportó un ataque de reemplazo de esa key; no confirmé su remedio).
— https://www.quicknode.com/guides/solana-development/spl-tokens/token-2022/confidential

Producción: **no encontré un usuario en producción confirmado post-reapertura.** Agora (AUSD) lo
usaba antes del apagón; el Solana Developer Platform lo tiene gated a devnet
(https://github.com/solana-foundation/solana-developer-platform/pull/1801).

**Para elote es un no estructural, por cuatro razones independientes:**
1. Es una extensión *de la mint*. USDC en Solana es SPL clásico y MEXe sandbox también (ver
   `2026-11-08-peso-series-mexe.md`). No hay forma de activarlo sobre los activos reales sin un
   token envuelto propio — otra capa de riesgo de emisor y de redención.
2. `Pool::nav` / `Pool::entry_nav` leen `vault.amount` en claro; todo §1.2 de ARCHITECTURE.md
   (precio dual, invariantes, caps) depende de eso. Un saldo cifrado lo rompe.
3. El vault es una PDA: `ConfigureAccount` y cada `Withdraw` exigen generar pruebas con la llave
   secreta ElGamal off-chain. Una PDA no tiene llave; el programa no puede mover un saldo
   confidencial sin delegar una llave a un humano — justo lo que `pool_signer!` evita.
4. Los depósitos y retiros al pool se ven de todos modos (montos públicos en la frontera).
La share mint sí es de elote y podría llevar la extensión, pero ocultar tenencias de inversionistas
KYC-eados no es un requisito que nadie haya pedido, y el kill-switch congelaría redenciones.

## 3. Light Protocol / ZK Compression

Qué es de verdad: **compresión de estado**, no privacidad. Las cuentas viven como hojas de árboles
Merkle (Poseidon) en el ledger; cada uso trae una validity proof Groth16 que un indexador (Photon,
Helius) genera. V2 (árboles batched, mainnet ene–mar 2026): ~250× más barato por actualización de
raíz, ~70% menos CU que V1. Una PDA de 100 bytes: ~0.0016 SOL normal vs ~0.00001 SOL comprimida.
Junio 10 2026: **Helius adquirió Light**; el equipo vuelve a "privacidad programable" y Helius
anuncia que *se discontinuarán funciones del Light Token SDK*.
— https://www.helius.dev/blog/light-protocol-acquisition ·
https://www.mintlify.com/Lightprotocol/light-protocol/concepts/zk-compression ·
https://www.spark.money/research/solana-zk-compression-onchain-cost-reduction

Para elote: el registro ya es **un hash por asiento** encadenado (`annotation_hash`, `registry.rs`)
dentro de cuentas existentes; no hay una masa de cuentas que comprimir. Un pilot con decenas de
pagarés paga rent trivial. Comprimir añadiría dependencia de un RPC/indexador para leer estado,
cuentas no legibles por otros programas con un account read normal, y riesgo de roadmap tras la
adquisición. Revisar sólo si algún día hay >10⁵ pagarés.

## 4. Privacidad programable y regulación

- **Arcium (MPC):** Mainnet Alpha desde 2026-02-02, permisionado, 4 operadores, componentes
  centralizados; TGE de ARX 2026-06-22. **C-SPL** (saldos cifrados para cualquier SPL, incluso
  sin Token-2022) se prometió para Q1 2026; **no encontré confirmación de lanzamiento** — sin
  verificar. Primera app: Umbra (USDC blindado, viewing keys).
  — https://arcium.substack.com/p/arcium-mainnet-alpha-is-live ·
  https://www.theblock.co/post/387564/arcium-launches-privacy-preserving-mainnet-alpha-on-solana-as-umbra-debuts-shielded-finance-layer ·
  https://solanacompass.com/learn/breakpoint-25/keynote-arcium-yannik-schrade
- **Privacy Cash:** pool blindado (circom + Groth16 on-chain sobre alt_bn128, ceremonia MPC),
  activo desde ago 2025, seis auditorías listadas (Accretion, HashCloak, Zigtur, Kriko, Sherlock,
  Veridise), volumen reportado $170–270M (cifras inconsistentes). Una fuente secundaria reporta
  que **pausó depósitos el 2026-11-04** tras el hack de Binance — sin verificar. Es la prueba de
  que circom + groth16-solana funciona en producción; también es un mixer.
  — https://www.mintlify.com/Privacy-Cash/privacy-cash/security/audits ·
  https://solanacompass.com/projects/privacy-cash
- **Hinkal** reporta $400M de volumen confidencial en Solana (abr 2026, fuente secundaria).
  — https://blockeden.xyz/blog/2026/04/19/hinkal-protocol-privacy-wallet-solana-400m-confidential-volume/
- **Elusiv:** cerrado en 2024; su equipo es Arcium.
- **Regulación:** OFAC deslistó Tornado Cash en mar 2025 (Van Loon, 5º Circuito), pero Roman Storm
  quedó condenado (ago 2025) por conspiración para operar un transmisor de dinero sin licencia; el
  retrial de lavado/sanciones se movió al 2027-04-26. La dirección de la industria es "privacidad
  con compliance" (auditor keys, viewing keys, proof-of-innocence). Para un vehículo de crédito
  mexicano con KYC (LFPIORPI, PLD) tocar un pool blindado es riesgo puro sin beneficio.
  — https://www.theblock.co/news/regulation/2026-08-26-tornado-cash-roman-storm-retrial-april-2027-412761

## 5. Circuitos y toolchains

| toolchain | verificador en Solana | estado | para un equipo chico |
|---|---|---|---|
| circom + snarkjs → `groth16-solana` | alt_bn128, <200k CU | auditado (Light v3); en producción (Privacy Cash, ZK Compression) | **el único camino serio**; requiere ceremonia por circuito |
| Noir → Sunspot (Reilabs, gnark) | genera programa verificador, ~500k CU, prueba 324–388 B | **sin auditar**; `sunspot setup` sin MPC (el prover guarda el trapdoor); pin a Noir beta | experimental; UltraHonk nativo no corre (faltan ops KZG) |
| gnark (Go) | vía Sunspot / verificadores Groth16 | maduro en EVM | sólo como backend de lo anterior |
| SP1 → `sp1-solana` | STARK envuelto en Groth16 | crate 0.1.0 (ago 2025) | probar ejecución Rust arbitraria; proving pesado (GPU/red de provers) |
| RISC Zero → `risc0-solana` / Bonsol | router + verificadores | auditado (zkSecurity 2025, Veridise oct 2025 p/ Boundless) pero el repo se declara no listo para producción | igual que SP1 |

— https://github.com/reilabs/sunspot · https://docs.rs/crate/sp1-solana/latest ·
https://reports.zksecurity.xyz/reports/risc0-solana-contracts ·
https://veridise.com/wp-content/uploads/2026/04/VAR-Boundless-251006-solana-verifier-V2.pdf ·
https://github.com/solana-foundation/noir-examples

Nota: verificar una prueba Groth16 sólo dice que el testigo satisface el circuito. El circuito
necesita su propia auditoría; es una superficie nueva del tamaño del programa actual.

## 6. ZK + identidad

- **SAS no usa ZK.** solana.com/privacy dice que SAS permite "confirmar que un cliente pasó KYC
  sin ver sus datos personales", pero el mecanismo es una atestación firmada por el emisor sobre
  una pubkey, no una prueba ZK. No encontré documentación de selective disclosure ZK en SAS.
- **zkTLS (Reclaim):** desplegado en Solana, pero la verificación on-chain es la firma de un
  *attestor* que vio la sesión TLS cifrada, no una prueba ZK verificada en cadena; confianza en el
  attestor. Paper IACR 2026/2187. — https://eprint.iacr.org/2026/2187.pdf ·
  https://solanacompass.com/projects/reclaim-protocol
- **Helius + Light** anuncian selective disclosure para auditores como *dirección*, no producto.

**Z1 (registro pseudónimo).** La pseudonimia ya la da la pubkey: hoy on-chain hay pubkeys y hashes,
no nombres. ZK no agrega nada a la pregunta *jurídica* (si una pubkey identifica suficientemente al
suscriptor/beneficiario para la LGTOC y para ejecutar el pagaré). Lo único adyacente — "esta pubkey
pasó KYC, sin PII" — es exactamente lo que hace una atestación SAS emitida tras Didit, **sin ZK**, y
eso ya está en el diseño.

**P-94 (borrado a 72 meses).** Nada en ZK borra una hash chain on-chain. El mecanismo correcto es
*crypto-shredding*: borrar off-chain los datos (y la sal del merkle del expediente) a los que el
hash se compromete, dejando un hash huérfano e inútil. elote ya está así por diseño (sólo hashes
salados on-chain). La pregunta para counsel es si un hash salado de datos borrados sigue siendo
dato personal — no técnica.

## 7. Veredicto para elote

| caso de uso | tecnología | madurez (nov 2026) | esfuerzo | ¿vale hoy? |
|---|---|---|---|---|
| Registro pseudónimo del pagaré (Z1) | pubkey + hashes salados (ya); SAS para "pasó KYC" | producción | cero | **ya resuelto sin ZK**; Z1 es pregunta legal |
| Montos privados (depósitos, pagos, saldos) | Token-2022 CT | re-habilitado jun 2026, sin adopción visible, kill-switch | alto + rompe NAV | **no, estructural** (USDC/MEXe sin la extensión; vault PDA sin llave; NAV en claro) |
| Montos privados vía MPC | Arcium C-SPL / Umbra | alpha permisionada; C-SPL sin confirmar | alto + dependencia de red MPC | **wait-and-see** (2027+) |
| Prueba de KYC/solvencia sin doxxear | SAS + Token ACL (sin ZK) | producción | ya planeado | **sí, sin ZK** |
| Prueba ZK de atributos (edad, ingreso, buró) | circom + groth16-solana / zkTLS | posible; nadie lo hace sobre credenciales mexicanas | alto (circuito + auditoría + ceremonia) | **no**; Didit + SAS basta para un pilot |
| Prueba de reservas / solvencia del pool | — | innecesario: vault y NAV ya son públicos | — | **no** (la transparencia es el producto) |
| Compresión de asientos/pagarés | Light ZK Compression V2 | producción, pero roadmap incierto post-Helius | medio + RPC dependiente | **no** hasta >10⁵ pagarés |
| Borrado P-94 | crypto-shredding off-chain | n/a | bajo | **sí, sin ZK** |
| Ocultar posiciones de inversionistas | CT en la share mint | idem CT | alto; kill-switch congela redenciones | **no**; nadie lo pidió |

**Recomendación:** no adoptar ZK en `elote_pool` v1. Dos cosas baratas: (a) documentar en el
brief a counsel que la pseudonimia de Z1 y el borrado de P-94 se resuelven con pubkeys + hashes
salados + borrado off-chain, sin criptografía nueva; (b) si algún día un regulador pide montos
privados, el camino a mirar es una auditor key (CT o C-SPL) sobre un token propio, y reevaluar
cuando haya un emisor de USDC/MEXe con la extensión activa y un año sin kill-switch. Si algún día
hace falta un circuito, circom + snarkjs + `groth16-solana` es el único stack con auditorías y
uso en producción en Solana.
