import {GolemService} from "@/service/base.ts";
import {APIService} from "@/service/client.ts";
import {MockService} from "@/service/mock.ts";


// export const SERVICE: GolemService = APIService;

export const SERVICE: GolemService = MockService;
