import * as fs from "fs";
import { parseArgs } from "util";
import { spawn } from 'child_process';

function runCommand(command: string, args: string[]) {
  return new Promise<void>((resolve, reject) => {
    const process = spawn(command, args);

    process.stdout.on('data', (data) => {
      console.log(`Output: ${data}`);
    });

    process.stderr.on('data', (data) => {
      console.error(`Error: ${data}`);
    });

    process.on('close', (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`Process exited with code ${code}`));
      }
});
  });
}

const { values, positionals } = parseArgs({
  args: process.argv,
  options: {
    vKeyPath: {
      type: "string",
    },
    proofPath: {
      type: "string",
    },
    outputPath: {
      type: "string",
    },
  },
  strict: true,
  allowPositionals: true,
});

interface HyleOutput {
  version: number;
  initial_state: number[];
  next_state: number[];
  identity: string;
  tx_hash: number[];
  payloads: number[];
  success: boolean;
}

function parseString(vector: string[]): string {
  let length = parseInt(vector.shift() as string);
  let resp = "";
  for (var i = 0; i < length; i += 1)
    resp += String.fromCharCode(parseInt(vector.shift() as string, 16));
  return resp;
}

function parseArray(vector: string[]): number[] {
  let length = parseInt(vector.shift() as string);
  let resp: number[] = [];
  for (var i = 0; i < length; i += 1)
    resp.push(parseInt(vector.shift() as string, 16));
  return resp;
}

function parsePayload(vector: string[]): number[] {
  let length = parseInt(vector.shift() as string);
  let payload: string = "[";
  for (var i = 0; i < length; i += 1)
    payload += BigInt(vector.shift() as string).toString() + " ";
  payload = payload.slice(0, -1) + "]"
  return Array.from(new TextEncoder().encode(payload));
}

function deserializePublicInputs<T>(publicInputs: string[]): HyleOutput {
  const version = parseInt(publicInputs.shift() as string);

  const initial_state = parseArray(publicInputs);
  const next_state = parseArray(publicInputs);
  const identity = parseString(publicInputs);
  const tx_hash = parseArray(publicInputs);
  const payloads = parsePayload(publicInputs);
  const success = parseInt(publicInputs.shift() as string) === 1;
  // We don't parse the rest, which correspond to programOutputs
  return {
    version,
    initial_state,
    next_state,
    identity,
    tx_hash,
    payloads,
    success,
  };
}

const command = 'bash';
const argsVerification = ['-c', `bb verify -p ${values.proofPath} -k ${values.vKeyPath}`];
await runCommand(command, argsVerification);
// Proof is considered valid

const argsProofAsFields = ['-c', `bb proof_as_fields -p ${values.proofPath} -k ${values.vKeyPath} -o ${values.outputPath}`];
await runCommand(command, argsProofAsFields);

let proofAsFields: string[] = JSON.parse(fs.readFileSync(values.outputPath));
const hyleOutput = deserializePublicInputs(proofAsFields);

var stringified_output = JSON.stringify(hyleOutput);

process.stdout.write(stringified_output);
process.exit(0);
