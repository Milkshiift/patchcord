import { AudioSharePatchbay } from '../patchcord.js';

const patchbay = new AudioSharePatchbay({
    command: './target/release/patchcord',
    sinkPrefix: 'TestAppShare',
    sinkDescription: 'Test Application Audio Share'
});

console.log("Has PipeWire:", await patchbay.hasPipeWire());
console.log(await patchbay.listShareableNodes(false));

await patchbay.dispose();