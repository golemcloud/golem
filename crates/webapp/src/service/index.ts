import {GolemService} from "@/service/base.ts";
import {APIService, Service} from "@/service/client.ts";
// import {MockService} from "@/service/mock.ts";


export const SERVICE: GolemService = APIService;

export const API = new Service();


// export const SERVICE: GolemService = MockService;
