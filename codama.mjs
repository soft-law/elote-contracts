// Generates the TypeScript client (clients/js/src/generated) from the Anchor IDL.
// Run `npm run codama` after every `anchor build`: it copies target/idl/elote_pool.json to idl/ first.
import {
  constantPdaSeedNodeFromString,
  createFromRoot,
  pdaNode,
  pdaValueNode,
  programIdValueNode,
  setInstructionAccountDefaultValuesVisitor,
} from "codama";
import { rootNodeFromAnchor } from "@codama/nodes-from-anchor";
import { renderVisitor } from "@codama/renderers-js";
import { readFileSync } from "node:fs";

const idl = JSON.parse(readFileSync(new URL("./idl/elote_pool.json", import.meta.url), "utf8"));
const codama = createFromRoot(rootNodeFromAnchor(idl));

// Anchor 1.x leaves the event-CPI accounts unresolved in the IDL: derive them like the Anchor client does.
codama.update(
  setInstructionAccountDefaultValuesVisitor([
    {
      account: "eventAuthority",
      defaultValue: pdaValueNode(
        pdaNode({ name: "eventAuthority", seeds: [constantPdaSeedNodeFromString("utf8", "__event_authority")] })
      ),
    },
    { account: "program", defaultValue: programIdValueNode() },
  ])
);

codama.accept(renderVisitor("clients/js", { deleteFolderBeforeRendering: true }));
