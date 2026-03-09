import { AudioSharePatchbay } from '../patchcord.js';

const patchbay = new AudioSharePatchbay({})

console.log("Has PipeWire:", await patchbay.hasPipeWire());
console.log(await patchbay.listShareableNodes(false));