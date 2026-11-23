import { Channel, invoke } from '@tauri-apps/api/core';
import type { RdpOptions } from './rdp';

interface FreerdpLaunchParams {
	host: string;
	port: number;
	username: string;
	domain: string | null;
	password: string | null;
	usePasswordRef: boolean;
	clipboard: 'text' | 'off' | null;
	graphicsPipeline: boolean;
	sharePath: string | null;
	program: string | null;
	rdp?: RdpOptions | null;
}

export function freerdpFind(program: string): Promise<string | null> {
	return invoke('freerdp_find', { program: program || null });
}

export function freerdpLaunch(
	params: FreerdpLaunchParams,
	sessionId: string,
	onError: (error: string) => void
): Promise<void> {
	const onFailure = new Channel<string>(onError);
	return invoke('freerdp_launch', { params, sessionId, onFailure });
}
