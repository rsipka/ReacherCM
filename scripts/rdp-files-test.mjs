/**
 * Tests the SFTP session an RDP session's Open file manager connects with
 * (#lib/sessions/rdp-files).
 *
 * Run: node --import ./scripts/resolve-lib.mjs scripts/rdp-files-test.mjs
 */

import { rdpFilesPort, rdpFilesSession, rdpFilesUsername } from '../src/lib/sessions/rdp-files.ts';

let failures = 0;
const check = (name, cond, extra = '') => {
	if (cond) console.log(`  PASS  ${name}`);
	else {
		failures++;
		console.log(`  FAIL  ${name} ${extra}`);
	}
};

check('plain user', rdpFilesUsername({ username: ' admin ' }) === 'admin');
check('domain in front', rdpFilesUsername({ username: 'admin', domain: 'CORP' }) === 'CORP\\admin');
check('blank domain ignored', rdpFilesUsername({ username: 'admin', domain: '  ' }) === 'admin');
check('user@domain kept', rdpFilesUsername({ username: 'admin@corp.example', domain: 'CORP' }) === 'admin@corp.example');
check('DOMAIN\\user kept', rdpFilesUsername({ username: 'OTHER\\admin', domain: 'CORP' }) === 'OTHER\\admin');
check('port 22 by default', rdpFilesPort({}) === 22 && rdpFilesPort({ rdp: { resolution: '1920x1080' } }) === 22);
check('own port', rdpFilesPort({ rdp: { files_port: 2222 } }) === 2222);

const rdp = {
	id: 's1',
	name: 'Desktop',
	host: 'desktop.example.com',
	port: 3389,
	username: 'admin',
	domain: 'CORP',
	auth_method: { type: 'Password', password: 'pw' },
	folder_id: null,
	tags: [],
	kind: 'rdp',
	share_path: '/home/me/share',
	rdp: { files_port: 2200, scale: 150 },
};
const s = rdpFilesSession(rdp);
check('ssh kind, sftp files', s.kind === 'ssh' && s.files === 'sftp');
check('same id and host', s.id === 's1' && s.host === 'desktop.example.com');
check('files port and login', s.port === 2200 && s.username === 'CORP\\admin');
check('password kept', s.auth_method.type === 'Password' && s.auth_method.password === 'pw');
check('no RDP settings', s.rdp === null && s.share_path === null);
check('the saved session untouched', rdp.kind === 'rdp' && rdp.port === 3389 && rdp.username === 'admin');

if (failures) {
	console.log(`\n${failures} failed`);
	process.exit(1);
}
console.log('\nAll passed');
