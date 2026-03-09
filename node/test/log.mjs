import { AudioSharePatchbay } from '../dist/index.js';

const patchbay = new AudioSharePatchbay({
    command: "./dist/patchcord-linux-x64"
})

console.log("Has PipeWire:", await patchbay.hasPipeWire());
console.log(await patchbay.listShareableNodes(false));