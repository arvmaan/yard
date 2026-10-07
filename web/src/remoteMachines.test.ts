import { describe,expect,it } from 'vitest'
import { buildAddMachineCommand,buildReconnectCommand,endpointKey,parseMachineInventory,parseRuntimeEndpoints,selectedEndpointAfterRefresh } from './remoteMachines'
import type { RuntimeEndpoint,RuntimeInventory } from './types'
const a='0123456789abcdef0123456789abcdef',b='abcdef0123456789abcdef0123456789'
const machine=(id:string,label='Box'):RuntimeEndpoint=>({endpoint:{kind:'machine',machine_id:id},label,enabled:true,connection_state:'reachable',capabilities:{inventory_read:true,mutations:false,terminal_streaming:false},sessions:[{name:'default',is_default:true,observed_running:true}]})
const local:RuntimeEndpoint={endpoint:{kind:'local'},label:'Local',enabled:true,connection_state:'reachable',capabilities:{inventory_read:true,mutations:true,terminal_streaming:true},sessions:[{name:'alpha',is_default:true,observed_running:true}]}
const inventory:RuntimeInventory={adapter:'herdr',session:'default',runtime_version:'0.9.3',protocol:19,observed_at_unix_ms:1,focus:{workspace_id:null,tab_id:null,pane_id:null},workspaces:[],tabs:[],panes:[],workers:[],child_agents:[]}
describe('remote machine boundary',()=>{
 it('parses capability and disconnected states',()=>{const x=parseRuntimeEndpoints({adapter:'herdr',endpoints:[local,machine(a),{...machine(b),enabled:false,connection_state:'disabled'}]});expect(x.endpoints.map(e=>endpointKey(e.endpoint))).toEqual(['local',`machine:${a}`,`machine:${b}`]);expect(x.endpoints[1].capabilities.mutations).toBe(false)})
 it('rejects malformed, duplicate, or missing Local endpoints',()=>{expect(()=>parseRuntimeEndpoints({adapter:'herdr',endpoints:[machine(a)]})).toThrow();expect(()=>parseRuntimeEndpoints({adapter:'herdr',endpoints:[local,machine(a),machine(a)]})).toThrow();expect(()=>parseRuntimeEndpoints({adapter:'herdr',endpoints:[local,{...machine(a),connection_state:'connected'}]})).toThrow()})
 it('preserves selection across rename and returns missing machine to Local',()=>{const k=`machine:${a}`;expect(selectedEndpointAfterRefresh(k,[local,machine(a,'Renamed')])).toBe(k);expect(selectedEndpointAfterRefresh(k,[local,machine(b)])).toBe('local')})
 it('requires matching endpoint-qualified inventory',()=>{expect(parseMachineInventory({endpoint:{kind:'machine',machine_id:a},inventory},a).inventory).toBe(inventory);expect(()=>parseMachineInventory({endpoint:{kind:'local'},inventory},a)).toThrow('identity mismatch');expect(()=>parseMachineInventory({endpoint:{kind:'machine',machine_id:b},inventory},a)).toThrow('identity mismatch')})
 it('keeps duplicate topology ids separate by endpoint',()=>expect(new Set([local,machine(a),machine(b)].map(e=>`${endpointKey(e.endpoint)}:w1:p1`)).size).toBe(3))
})
describe('command construction',()=>{
 it('quotes spaces, quotes, and option-looking inputs',()=>{expect(buildAddMachineCommand({sshTarget:'-host name',label:"Builder's box",remoteSession:'release one'})).toBe(`herdr machine add --label='Builder'"'"'s box' --remote-session='release one' -- '-host name'`);expect(buildAddMachineCommand({sshTarget:'user@host',label:'',remoteSession:''})).toBe("herdr machine add -- 'user@host'")})
 it('rejects newline and NUL injection',()=>{expect(()=>buildAddMachineCommand({sshTarget:'host\ncommand',label:'',remoteSession:''})).toThrow('newlines');expect(()=>buildAddMachineCommand({sshTarget:'host',label:'bad\0label',remoteSession:''})).toThrow('NUL');expect(()=>buildAddMachineCommand({sshTarget:'host',label:'',remoteSession:'bad\rname'})).toThrow('newlines')})
 it('reconnects only by validated stable id',()=>{expect(buildReconnectCommand(a)).toBe(`herdr machine reconnect '${a}'`);expect(()=>buildReconnectCommand('--label')).toThrow('validated stable machine ID')})
})
