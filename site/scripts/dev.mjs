import { spawn } from "node:child_process";

const children = [
	{ name: "11ty", command: "pnpm", args: [ "dev:11ty" ] },
	{ name: "css", command: "pnpm", args: [ "dev:css" ] }
];

const running = children.map( ( child ) => {
	const processHandle = spawn( child.command, child.args, {
		stdio: "inherit",
		shell: true
	} );

	processHandle.on( "exit", ( code ) => {
		if ( code !== 0 ) {
			process.exitCode = code ?? 1;
			shutdown( "SIGTERM" );
		}
	} );

	return processHandle;
} );

const shutdown = ( signal ) => {
	for ( const child of running ) {
		if ( !child.killed ) {
			child.kill( signal );
		}
	}
};

process.on( "SIGINT", () => shutdown( "SIGINT" ) );
process.on( "SIGTERM", () => shutdown( "SIGTERM" ) );
