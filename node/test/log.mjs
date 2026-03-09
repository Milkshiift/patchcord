import { AudioSharePatchbay } from '../dist/index.js';

const patchbay = new AudioSharePatchbay({
    command: "./target/x86_64-unknown-linux-gnu/release/patchcord"
})

console.log("Has PipeWire:", await patchbay.hasPipeWire());
console.log(await patchbay.listShareableNodes(false));